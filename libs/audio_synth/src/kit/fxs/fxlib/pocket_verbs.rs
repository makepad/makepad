// Ported from PocketVerbs (PocketVerbsProc.cpp, PocketVerbs.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Pocket Verbs (sub-effect id 24): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    SlotDef::choice("Type", &["Chamber", "Spring", "Tiled", "Room", "Stretch", "Zarathu"], 0.0),
    SlotDef::float("Size", 0.0, 1.0, 0.5, crate::kit::core::SlotUnit::None),
    SlotDef::float("Gating", 0.0, 1.0, 0.0, crate::kit::core::SlotUnit::None),
    SlotDef::float("Dry/Wet", 0.0, 1.0, 0.5, crate::kit::core::SlotUnit::None),
];

#[allow(dead_code)]
pub struct PocketVerbs {
    p: [f32; 4],
    sr: f32,
    count: i32,
    d_ar: [f64; 7],
    d_br: [f64; 7],
    d_cr: [f64; 7],
    d_dr: [f64; 7],
    d_er: [f64; 7],
    d_fr: [f64; 7],
    d_gr: [f64; 7],
    d_hr: [f64; 7],
    d_ir: [f64; 7],
    d_jr: [f64; 7],
    d_kr: [f64; 7],
    d_lr: [f64; 7],
    d_mr: [f64; 7],
    d_nr: [f64; 7],
    d_or: [f64; 7],
    d_pr: [f64; 7],
    d_qr: [f64; 7],
    d_rr: [f64; 7],
    d_sr: [f64; 7],
    d_tr: [f64; 7],
    d_ur: [f64; 7],
    d_vr: [f64; 7],
    d_wr: [f64; 7],
    d_xr: [f64; 7],
    d_yr: [f64; 7],
    d_zr: [f64; 7],
    a_ar: Vec<f64>,
    a_br: Vec<f64>,
    a_cr: Vec<f64>,
    a_dr: Vec<f64>,
    a_er: Vec<f64>,
    a_fr: Vec<f64>,
    a_gr: Vec<f64>,
    a_hr: Vec<f64>,
    a_ir: Vec<f64>,
    a_jr: Vec<f64>,
    a_kr: Vec<f64>,
    a_lr: Vec<f64>,
    a_mr: Vec<f64>,
    a_nr: Vec<f64>,
    a_or: Vec<f64>,
    a_pr: Vec<f64>,
    a_qr: Vec<f64>,
    a_rr: Vec<f64>,
    a_sr: Vec<f64>,
    a_tr: Vec<f64>,
    a_ur: Vec<f64>,
    a_vr: Vec<f64>,
    a_wr: Vec<f64>,
    a_xr: Vec<f64>,
    a_yr: Vec<f64>,
    a_zr: Vec<f64>,
    o_ar: Vec<f64>,
    o_br: Vec<f64>,
    o_cr: Vec<f64>,
    o_dr: Vec<f64>,
    o_er: Vec<f64>,
    o_fr: Vec<f64>,
    o_gr: Vec<f64>,
    o_hr: Vec<f64>,
    o_ir: Vec<f64>,
    o_jr: Vec<f64>,
    o_kr: Vec<f64>,
    o_lr: Vec<f64>,
    o_mr: Vec<f64>,
    o_nr: Vec<f64>,
    o_or: Vec<f64>,
    o_pr: Vec<f64>,
    o_qr: Vec<f64>,
    o_rr: Vec<f64>,
    o_sr: Vec<f64>,
    o_tr: Vec<f64>,
    o_ur: Vec<f64>,
    o_vr: Vec<f64>,
    o_wr: Vec<f64>,
    o_xr: Vec<f64>,
    o_yr: Vec<f64>,
    o_zr: Vec<f64>,
    d_al: [f64; 7],
    d_bl: [f64; 7],
    d_cl: [f64; 7],
    d_dl: [f64; 7],
    d_el: [f64; 7],
    d_fl: [f64; 7],
    d_gl: [f64; 7],
    d_hl: [f64; 7],
    d_il: [f64; 7],
    d_jl: [f64; 7],
    d_kl: [f64; 7],
    d_ll: [f64; 7],
    d_ml: [f64; 7],
    d_nl: [f64; 7],
    d_ol: [f64; 7],
    d_pl: [f64; 7],
    d_ql: [f64; 7],
    d_rl: [f64; 7],
    d_sl: [f64; 7],
    d_tl: [f64; 7],
    d_ul: [f64; 7],
    d_vl: [f64; 7],
    d_wl: [f64; 7],
    d_xl: [f64; 7],
    d_yl: [f64; 7],
    d_zl: [f64; 7],
    a_al: Vec<f64>,
    a_bl: Vec<f64>,
    a_cl: Vec<f64>,
    a_dl: Vec<f64>,
    a_el: Vec<f64>,
    a_fl: Vec<f64>,
    a_gl: Vec<f64>,
    a_hl: Vec<f64>,
    a_il: Vec<f64>,
    a_jl: Vec<f64>,
    a_kl: Vec<f64>,
    a_ll: Vec<f64>,
    a_ml: Vec<f64>,
    a_nl: Vec<f64>,
    a_ol: Vec<f64>,
    a_pl: Vec<f64>,
    a_ql: Vec<f64>,
    a_rl: Vec<f64>,
    a_sl: Vec<f64>,
    a_tl: Vec<f64>,
    a_ul: Vec<f64>,
    a_vl: Vec<f64>,
    a_wl: Vec<f64>,
    a_xl: Vec<f64>,
    a_yl: Vec<f64>,
    a_zl: Vec<f64>,
    o_al: Vec<f64>,
    o_bl: Vec<f64>,
    o_cl: Vec<f64>,
    o_dl: Vec<f64>,
    o_el: Vec<f64>,
    o_fl: Vec<f64>,
    o_gl: Vec<f64>,
    o_hl: Vec<f64>,
    o_il: Vec<f64>,
    o_jl: Vec<f64>,
    o_kl: Vec<f64>,
    o_ll: Vec<f64>,
    o_ml: Vec<f64>,
    o_nl: Vec<f64>,
    o_ol: Vec<f64>,
    o_pl: Vec<f64>,
    o_ql: Vec<f64>,
    o_rl: Vec<f64>,
    o_sl: Vec<f64>,
    o_tl: Vec<f64>,
    o_ul: Vec<f64>,
    o_vl: Vec<f64>,
    o_wl: Vec<f64>,
    o_xl: Vec<f64>,
    o_yl: Vec<f64>,
    o_zl: Vec<f64>,
    out_al: i32,
    alp_al: i32,
    out_bl: i32,
    alp_bl: i32,
    out_cl: i32,
    alp_cl: i32,
    out_dl: i32,
    alp_dl: i32,
    out_el: i32,
    alp_el: i32,
    out_fl: i32,
    alp_fl: i32,
    out_gl: i32,
    alp_gl: i32,
    out_hl: i32,
    alp_hl: i32,
    out_il: i32,
    alp_il: i32,
    out_jl: i32,
    alp_jl: i32,
    out_kl: i32,
    alp_kl: i32,
    out_ll: i32,
    alp_ll: i32,
    out_ml: i32,
    alp_ml: i32,
    out_nl: i32,
    alp_nl: i32,
    out_ol: i32,
    alp_ol: i32,
    out_pl: i32,
    alp_pl: i32,
    out_ql: i32,
    alp_ql: i32,
    out_rl: i32,
    alp_rl: i32,
    out_sl: i32,
    alp_sl: i32,
    out_tl: i32,
    alp_tl: i32,
    out_ul: i32,
    alp_ul: i32,
    out_vl: i32,
    alp_vl: i32,
    out_wl: i32,
    alp_wl: i32,
    out_xl: i32,
    alp_xl: i32,
    out_yl: i32,
    alp_yl: i32,
    out_zl: i32,
    alp_zl: i32,
    out_ar: i32,
    alp_ar: i32,
    maxdelay_a: i32,
    delay_a: i32,
    out_br: i32,
    alp_br: i32,
    maxdelay_b: i32,
    delay_b: i32,
    out_cr: i32,
    alp_cr: i32,
    maxdelay_c: i32,
    delay_c: i32,
    out_dr: i32,
    alp_dr: i32,
    maxdelay_d: i32,
    delay_d: i32,
    out_er: i32,
    alp_er: i32,
    maxdelay_e: i32,
    delay_e: i32,
    out_fr: i32,
    alp_fr: i32,
    maxdelay_f: i32,
    delay_f: i32,
    out_gr: i32,
    alp_gr: i32,
    maxdelay_g: i32,
    delay_g: i32,
    out_hr: i32,
    alp_hr: i32,
    maxdelay_h: i32,
    delay_h: i32,
    out_ir: i32,
    alp_ir: i32,
    maxdelay_i: i32,
    delay_i: i32,
    out_jr: i32,
    alp_jr: i32,
    maxdelay_j: i32,
    delay_j: i32,
    out_kr: i32,
    alp_kr: i32,
    maxdelay_k: i32,
    delay_k: i32,
    out_lr: i32,
    alp_lr: i32,
    maxdelay_l: i32,
    delay_l: i32,
    out_mr: i32,
    alp_mr: i32,
    maxdelay_m: i32,
    delay_m: i32,
    out_nr: i32,
    alp_nr: i32,
    maxdelay_n: i32,
    delay_n: i32,
    out_or: i32,
    alp_or: i32,
    maxdelay_o: i32,
    delay_o: i32,
    out_pr: i32,
    alp_pr: i32,
    maxdelay_p: i32,
    delay_p: i32,
    out_qr: i32,
    alp_qr: i32,
    maxdelay_q: i32,
    delay_q: i32,
    out_rr: i32,
    alp_rr: i32,
    maxdelay_r: i32,
    delay_r: i32,
    out_sr: i32,
    alp_sr: i32,
    maxdelay_s: i32,
    delay_s: i32,
    out_tr: i32,
    alp_tr: i32,
    maxdelay_t: i32,
    delay_t: i32,
    out_ur: i32,
    alp_ur: i32,
    maxdelay_u: i32,
    delay_u: i32,
    out_vr: i32,
    alp_vr: i32,
    maxdelay_v: i32,
    delay_v: i32,
    out_wr: i32,
    alp_wr: i32,
    maxdelay_w: i32,
    delay_w: i32,
    out_xr: i32,
    alp_xr: i32,
    maxdelay_x: i32,
    delay_x: i32,
    out_yr: i32,
    alp_yr: i32,
    maxdelay_y: i32,
    delay_y: i32,
    out_zr: i32,
    alp_zr: i32,
    maxdelay_z: i32,
    delay_z: i32,
    saved_roomsize: f64,
    countdown: i32,
    peak_l: f64,
    peak_r: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl PocketVerbs {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<PocketVerbs> {
        let mut s = Box::new(PocketVerbs {
            p: [0.0, 0.5, 0.0, 0.5],
            sr: sr as f32,
            count: 0,
            d_ar: [0.0; 7],
            d_br: [0.0; 7],
            d_cr: [0.0; 7],
            d_dr: [0.0; 7],
            d_er: [0.0; 7],
            d_fr: [0.0; 7],
            d_gr: [0.0; 7],
            d_hr: [0.0; 7],
            d_ir: [0.0; 7],
            d_jr: [0.0; 7],
            d_kr: [0.0; 7],
            d_lr: [0.0; 7],
            d_mr: [0.0; 7],
            d_nr: [0.0; 7],
            d_or: [0.0; 7],
            d_pr: [0.0; 7],
            d_qr: [0.0; 7],
            d_rr: [0.0; 7],
            d_sr: [0.0; 7],
            d_tr: [0.0; 7],
            d_ur: [0.0; 7],
            d_vr: [0.0; 7],
            d_wr: [0.0; 7],
            d_xr: [0.0; 7],
            d_yr: [0.0; 7],
            d_zr: [0.0; 7],
            a_ar: vec![0.0; 15150],
            a_br: vec![0.0; 14618],
            a_cr: vec![0.0; 14358],
            a_dr: vec![0.0; 13818],
            a_er: vec![0.0; 13562],
            a_fr: vec![0.0; 13046],
            a_gr: vec![0.0; 11966],
            a_hr: vec![0.0; 11130],
            a_ir: vec![0.0; 10598],
            a_jr: vec![0.0; 9810],
            a_kr: vec![0.0; 9522],
            a_lr: vec![0.0; 8982],
            a_mr: vec![0.0; 8786],
            a_nr: vec![0.0; 8462],
            a_or: vec![0.0; 8310],
            a_pr: vec![0.0; 7982],
            a_qr: vec![0.0; 7322],
            a_rr: vec![0.0; 6818],
            a_sr: vec![0.0; 6506],
            a_tr: vec![0.0; 6002],
            a_ur: vec![0.0; 5838],
            a_vr: vec![0.0; 5502],
            a_wr: vec![0.0; 5010],
            a_xr: vec![0.0; 4850],
            a_yr: vec![0.0; 4296],
            a_zr: vec![0.0; 4180],
            o_ar: vec![0.0; 15150],
            o_br: vec![0.0; 14618],
            o_cr: vec![0.0; 14358],
            o_dr: vec![0.0; 13818],
            o_er: vec![0.0; 13562],
            o_fr: vec![0.0; 13046],
            o_gr: vec![0.0; 11966],
            o_hr: vec![0.0; 11130],
            o_ir: vec![0.0; 10598],
            o_jr: vec![0.0; 9810],
            o_kr: vec![0.0; 9522],
            o_lr: vec![0.0; 8982],
            o_mr: vec![0.0; 8786],
            o_nr: vec![0.0; 8462],
            o_or: vec![0.0; 8310],
            o_pr: vec![0.0; 7982],
            o_qr: vec![0.0; 7322],
            o_rr: vec![0.0; 6818],
            o_sr: vec![0.0; 6506],
            o_tr: vec![0.0; 6002],
            o_ur: vec![0.0; 5838],
            o_vr: vec![0.0; 5502],
            o_wr: vec![0.0; 5010],
            o_xr: vec![0.0; 4850],
            o_yr: vec![0.0; 4296],
            o_zr: vec![0.0; 4180],
            d_al: [0.0; 7],
            d_bl: [0.0; 7],
            d_cl: [0.0; 7],
            d_dl: [0.0; 7],
            d_el: [0.0; 7],
            d_fl: [0.0; 7],
            d_gl: [0.0; 7],
            d_hl: [0.0; 7],
            d_il: [0.0; 7],
            d_jl: [0.0; 7],
            d_kl: [0.0; 7],
            d_ll: [0.0; 7],
            d_ml: [0.0; 7],
            d_nl: [0.0; 7],
            d_ol: [0.0; 7],
            d_pl: [0.0; 7],
            d_ql: [0.0; 7],
            d_rl: [0.0; 7],
            d_sl: [0.0; 7],
            d_tl: [0.0; 7],
            d_ul: [0.0; 7],
            d_vl: [0.0; 7],
            d_wl: [0.0; 7],
            d_xl: [0.0; 7],
            d_yl: [0.0; 7],
            d_zl: [0.0; 7],
            a_al: vec![0.0; 15150],
            a_bl: vec![0.0; 14618],
            a_cl: vec![0.0; 14358],
            a_dl: vec![0.0; 13818],
            a_el: vec![0.0; 13562],
            a_fl: vec![0.0; 13046],
            a_gl: vec![0.0; 11966],
            a_hl: vec![0.0; 11130],
            a_il: vec![0.0; 10598],
            a_jl: vec![0.0; 9810],
            a_kl: vec![0.0; 9522],
            a_ll: vec![0.0; 8982],
            a_ml: vec![0.0; 8786],
            a_nl: vec![0.0; 8462],
            a_ol: vec![0.0; 8310],
            a_pl: vec![0.0; 7982],
            a_ql: vec![0.0; 7322],
            a_rl: vec![0.0; 6818],
            a_sl: vec![0.0; 6506],
            a_tl: vec![0.0; 6002],
            a_ul: vec![0.0; 5838],
            a_vl: vec![0.0; 5502],
            a_wl: vec![0.0; 5010],
            a_xl: vec![0.0; 4850],
            a_yl: vec![0.0; 4296],
            a_zl: vec![0.0; 4180],
            o_al: vec![0.0; 15150],
            o_bl: vec![0.0; 14618],
            o_cl: vec![0.0; 14358],
            o_dl: vec![0.0; 13818],
            o_el: vec![0.0; 13562],
            o_fl: vec![0.0; 13046],
            o_gl: vec![0.0; 11966],
            o_hl: vec![0.0; 11130],
            o_il: vec![0.0; 10598],
            o_jl: vec![0.0; 9810],
            o_kl: vec![0.0; 9522],
            o_ll: vec![0.0; 8982],
            o_ml: vec![0.0; 8786],
            o_nl: vec![0.0; 8462],
            o_ol: vec![0.0; 8310],
            o_pl: vec![0.0; 7982],
            o_ql: vec![0.0; 7322],
            o_rl: vec![0.0; 6818],
            o_sl: vec![0.0; 6506],
            o_tl: vec![0.0; 6002],
            o_ul: vec![0.0; 5838],
            o_vl: vec![0.0; 5502],
            o_wl: vec![0.0; 5010],
            o_xl: vec![0.0; 4850],
            o_yl: vec![0.0; 4296],
            o_zl: vec![0.0; 4180],
            out_al: 0,
            alp_al: 0,
            out_bl: 0,
            alp_bl: 0,
            out_cl: 0,
            alp_cl: 0,
            out_dl: 0,
            alp_dl: 0,
            out_el: 0,
            alp_el: 0,
            out_fl: 0,
            alp_fl: 0,
            out_gl: 0,
            alp_gl: 0,
            out_hl: 0,
            alp_hl: 0,
            out_il: 0,
            alp_il: 0,
            out_jl: 0,
            alp_jl: 0,
            out_kl: 0,
            alp_kl: 0,
            out_ll: 0,
            alp_ll: 0,
            out_ml: 0,
            alp_ml: 0,
            out_nl: 0,
            alp_nl: 0,
            out_ol: 0,
            alp_ol: 0,
            out_pl: 0,
            alp_pl: 0,
            out_ql: 0,
            alp_ql: 0,
            out_rl: 0,
            alp_rl: 0,
            out_sl: 0,
            alp_sl: 0,
            out_tl: 0,
            alp_tl: 0,
            out_ul: 0,
            alp_ul: 0,
            out_vl: 0,
            alp_vl: 0,
            out_wl: 0,
            alp_wl: 0,
            out_xl: 0,
            alp_xl: 0,
            out_yl: 0,
            alp_yl: 0,
            out_zl: 0,
            alp_zl: 0,
            out_ar: 0,
            alp_ar: 0,
            maxdelay_a: 0,
            delay_a: 0,
            out_br: 0,
            alp_br: 0,
            maxdelay_b: 0,
            delay_b: 0,
            out_cr: 0,
            alp_cr: 0,
            maxdelay_c: 0,
            delay_c: 0,
            out_dr: 0,
            alp_dr: 0,
            maxdelay_d: 0,
            delay_d: 0,
            out_er: 0,
            alp_er: 0,
            maxdelay_e: 0,
            delay_e: 0,
            out_fr: 0,
            alp_fr: 0,
            maxdelay_f: 0,
            delay_f: 0,
            out_gr: 0,
            alp_gr: 0,
            maxdelay_g: 0,
            delay_g: 0,
            out_hr: 0,
            alp_hr: 0,
            maxdelay_h: 0,
            delay_h: 0,
            out_ir: 0,
            alp_ir: 0,
            maxdelay_i: 0,
            delay_i: 0,
            out_jr: 0,
            alp_jr: 0,
            maxdelay_j: 0,
            delay_j: 0,
            out_kr: 0,
            alp_kr: 0,
            maxdelay_k: 0,
            delay_k: 0,
            out_lr: 0,
            alp_lr: 0,
            maxdelay_l: 0,
            delay_l: 0,
            out_mr: 0,
            alp_mr: 0,
            maxdelay_m: 0,
            delay_m: 0,
            out_nr: 0,
            alp_nr: 0,
            maxdelay_n: 0,
            delay_n: 0,
            out_or: 0,
            alp_or: 0,
            maxdelay_o: 0,
            delay_o: 0,
            out_pr: 0,
            alp_pr: 0,
            maxdelay_p: 0,
            delay_p: 0,
            out_qr: 0,
            alp_qr: 0,
            maxdelay_q: 0,
            delay_q: 0,
            out_rr: 0,
            alp_rr: 0,
            maxdelay_r: 0,
            delay_r: 0,
            out_sr: 0,
            alp_sr: 0,
            maxdelay_s: 0,
            delay_s: 0,
            out_tr: 0,
            alp_tr: 0,
            maxdelay_t: 0,
            delay_t: 0,
            out_ur: 0,
            alp_ur: 0,
            maxdelay_u: 0,
            delay_u: 0,
            out_vr: 0,
            alp_vr: 0,
            maxdelay_v: 0,
            delay_v: 0,
            out_wr: 0,
            alp_wr: 0,
            maxdelay_w: 0,
            delay_w: 0,
            out_xr: 0,
            alp_xr: 0,
            maxdelay_x: 0,
            delay_x: 0,
            out_yr: 0,
            alp_yr: 0,
            maxdelay_y: 0,
            delay_y: 0,
            out_zr: 0,
            alp_zr: 0,
            maxdelay_z: 0,
            delay_z: 0,
            saved_roomsize: 0.0,
            countdown: 0,
            peak_l: 0.0,
            peak_r: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.out_al = 1;
        s.alp_al = 1;
        s.out_bl = 1;
        s.alp_bl = 1;
        s.out_cl = 1;
        s.alp_cl = 1;
        s.out_dl = 1;
        s.alp_dl = 1;
        s.out_el = 1;
        s.alp_el = 1;
        s.out_fl = 1;
        s.alp_fl = 1;
        s.out_gl = 1;
        s.alp_gl = 1;
        s.out_hl = 1;
        s.alp_hl = 1;
        s.out_il = 1;
        s.alp_il = 1;
        s.out_jl = 1;
        s.alp_jl = 1;
        s.out_kl = 1;
        s.alp_kl = 1;
        s.out_ll = 1;
        s.alp_ll = 1;
        s.out_ml = 1;
        s.alp_ml = 1;
        s.out_nl = 1;
        s.alp_nl = 1;
        s.out_ol = 1;
        s.alp_ol = 1;
        s.out_pl = 1;
        s.alp_pl = 1;
        s.out_ql = 1;
        s.alp_ql = 1;
        s.out_rl = 1;
        s.alp_rl = 1;
        s.out_sl = 1;
        s.alp_sl = 1;
        s.out_tl = 1;
        s.alp_tl = 1;
        s.out_ul = 1;
        s.alp_ul = 1;
        s.out_vl = 1;
        s.alp_vl = 1;
        s.out_wl = 1;
        s.alp_wl = 1;
        s.out_xl = 1;
        s.alp_xl = 1;
        s.out_yl = 1;
        s.alp_yl = 1;
        s.out_zl = 1;
        s.alp_zl = 1;
        s.out_ar = 1;
        s.alp_ar = 1;
        s.delay_a = 4;
        s.maxdelay_a = 7573;
        s.out_br = 1;
        s.alp_br = 1;
        s.delay_b = 4;
        s.maxdelay_b = 7307;
        s.out_cr = 1;
        s.alp_cr = 1;
        s.delay_c = 4;
        s.maxdelay_c = 7177;
        s.out_dr = 1;
        s.alp_dr = 1;
        s.delay_d = 4;
        s.maxdelay_d = 6907;
        s.out_er = 1;
        s.alp_er = 1;
        s.delay_e = 4;
        s.maxdelay_e = 6779;
        s.out_fr = 1;
        s.alp_fr = 1;
        s.delay_f = 4;
        s.maxdelay_f = 6521;
        s.out_gr = 1;
        s.alp_gr = 1;
        s.delay_g = 4;
        s.maxdelay_g = 5981;
        s.out_hr = 1;
        s.alp_hr = 1;
        s.delay_h = 4;
        s.maxdelay_h = 5563;
        s.out_ir = 1;
        s.alp_ir = 1;
        s.delay_i = 4;
        s.maxdelay_i = 5297;
        s.out_jr = 1;
        s.alp_jr = 1;
        s.delay_j = 4;
        s.maxdelay_j = 4903;
        s.out_kr = 1;
        s.alp_kr = 1;
        s.delay_k = 4;
        s.maxdelay_k = 4759;
        s.out_lr = 1;
        s.alp_lr = 1;
        s.delay_l = 4;
        s.maxdelay_l = 4489;
        s.out_mr = 1;
        s.alp_mr = 1;
        s.delay_m = 4;
        s.maxdelay_m = 4391;
        s.out_nr = 1;
        s.alp_nr = 1;
        s.delay_n = 4;
        s.maxdelay_n = 4229;
        s.out_or = 1;
        s.alp_or = 1;
        s.delay_o = 4;
        s.maxdelay_o = 4153;
        s.out_pr = 1;
        s.alp_pr = 1;
        s.delay_p = 4;
        s.maxdelay_p = 3989;
        s.out_qr = 1;
        s.alp_qr = 1;
        s.delay_q = 4;
        s.maxdelay_q = 3659;
        s.out_rr = 1;
        s.alp_rr = 1;
        s.delay_r = 4;
        s.maxdelay_r = 3407;
        s.out_sr = 1;
        s.alp_sr = 1;
        s.delay_s = 4;
        s.maxdelay_s = 3251;
        s.out_tr = 1;
        s.alp_tr = 1;
        s.delay_t = 4;
        s.maxdelay_t = 2999;
        s.out_ur = 1;
        s.alp_ur = 1;
        s.delay_u = 4;
        s.maxdelay_u = 2917;
        s.out_vr = 1;
        s.alp_vr = 1;
        s.delay_v = 4;
        s.maxdelay_v = 2749;
        s.out_wr = 1;
        s.alp_wr = 1;
        s.delay_w = 4;
        s.maxdelay_w = 2503;
        s.out_xr = 1;
        s.alp_xr = 1;
        s.delay_x = 4;
        s.maxdelay_x = 2423;
        s.out_yr = 1;
        s.alp_yr = 1;
        s.delay_y = 4;
        s.maxdelay_y = 2146;
        s.out_zr = 1;
        s.alp_zr = 1;
        s.delay_z = 4;
        s.maxdelay_z = 2088;
        s.saved_roomsize = -1.0;
        s.countdown = -1;
        s.peak_l = 1.0;
        s.peak_r = 1.0;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for PocketVerbs {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut verbtype: i32 = (s.p[0] as f64 * 5.999) as i32 + 1;
        let mut roomsize: f64 = (s.p[1] as f64).powf(2.0) * 1.9 + 0.1;
        let mut release: f64 = 0.00008 * (s.p[2] as f64).powf(3.0);
        if release == 0.0 {
            s.peak_l = 1.0;
            s.peak_r = 1.0;
        }
        let mut wetnesstarget: f64 = s.p[3] as f64;
        let mut dryness: f64 = 1.0 - wetnesstarget;
        let mut wetness: f64 = wetnesstarget;
        let mut constallpass: f64 = 0.618033988749894848204586;
        let mut allpasstemp: i32 = 0;
        let mut count: i32 = 0;
        let mut max: i32 = 70;
        let mut bridgerectifier: f64 = 0.0;
        let mut gain: f64 = 0.5 + wetnesstarget * 0.5;
        if s.saved_roomsize != roomsize {
            s.saved_roomsize = roomsize;
            s.countdown = 26;
        }
        if s.countdown > 0 {
            match s.countdown {
                1 => {
                    s.delay_a = (s.maxdelay_a as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_a != count && s.delay_a % count == 0 {
                                s.delay_a += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_a > s.maxdelay_a {
                        s.delay_a = s.maxdelay_a;
                    }
                    {
                        count = s.alp_al;
                        while count < 15149 {
                            s.a_al[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_al;
                        while count < 15149 {
                            s.o_al[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                2 => {
                    s.delay_b = (s.maxdelay_b as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_b != count && s.delay_b % count == 0 {
                                s.delay_b += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_b > s.maxdelay_b {
                        s.delay_b = s.maxdelay_b;
                    }
                    {
                        count = s.alp_bl;
                        while count < 14617 {
                            s.a_bl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_bl;
                        while count < 14617 {
                            s.o_bl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                3 => {
                    s.delay_c = (s.maxdelay_c as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_c != count && s.delay_c % count == 0 {
                                s.delay_c += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_c > s.maxdelay_c {
                        s.delay_c = s.maxdelay_c;
                    }
                    {
                        count = s.alp_cl;
                        while count < 14357 {
                            s.a_cl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_cl;
                        while count < 14357 {
                            s.o_cl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                4 => {
                    s.delay_d = (s.maxdelay_d as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_d != count && s.delay_d % count == 0 {
                                s.delay_d += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_d > s.maxdelay_d {
                        s.delay_d = s.maxdelay_d;
                    }
                    {
                        count = s.alp_dl;
                        while count < 13817 {
                            s.a_dl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_dl;
                        while count < 13817 {
                            s.o_dl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                5 => {
                    s.delay_e = (s.maxdelay_e as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_e != count && s.delay_e % count == 0 {
                                s.delay_e += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_e > s.maxdelay_e {
                        s.delay_e = s.maxdelay_e;
                    }
                    {
                        count = s.alp_el;
                        while count < 13561 {
                            s.a_el[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_el;
                        while count < 13561 {
                            s.o_el[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                6 => {
                    s.delay_f = (s.maxdelay_f as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_f != count && s.delay_f % count == 0 {
                                s.delay_f += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_f > s.maxdelay_f {
                        s.delay_f = s.maxdelay_f;
                    }
                    {
                        count = s.alp_fl;
                        while count < 13045 {
                            s.a_fl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_fl;
                        while count < 13045 {
                            s.o_fl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                7 => {
                    s.delay_g = (s.maxdelay_g as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_g != count && s.delay_g % count == 0 {
                                s.delay_g += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_g > s.maxdelay_g {
                        s.delay_g = s.maxdelay_g;
                    }
                    {
                        count = s.alp_gl;
                        while count < 11965 {
                            s.a_gl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_gl;
                        while count < 11965 {
                            s.o_gl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                8 => {
                    s.delay_h = (s.maxdelay_h as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_h != count && s.delay_h % count == 0 {
                                s.delay_h += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_h > s.maxdelay_h {
                        s.delay_h = s.maxdelay_h;
                    }
                    {
                        count = s.alp_hl;
                        while count < 11129 {
                            s.a_hl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_hl;
                        while count < 11129 {
                            s.o_hl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                9 => {
                    s.delay_i = (s.maxdelay_i as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_i != count && s.delay_i % count == 0 {
                                s.delay_i += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_i > s.maxdelay_i {
                        s.delay_i = s.maxdelay_i;
                    }
                    {
                        count = s.alp_il;
                        while count < 10597 {
                            s.a_il[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_il;
                        while count < 10597 {
                            s.o_il[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                10 => {
                    s.delay_j = (s.maxdelay_j as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_j != count && s.delay_j % count == 0 {
                                s.delay_j += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_j > s.maxdelay_j {
                        s.delay_j = s.maxdelay_j;
                    }
                    {
                        count = s.alp_jl;
                        while count < 9809 {
                            s.a_jl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_jl;
                        while count < 9809 {
                            s.o_jl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                11 => {
                    s.delay_k = (s.maxdelay_k as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_k != count && s.delay_k % count == 0 {
                                s.delay_k += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_k > s.maxdelay_k {
                        s.delay_k = s.maxdelay_k;
                    }
                    {
                        count = s.alp_kl;
                        while count < 9521 {
                            s.a_kl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_kl;
                        while count < 9521 {
                            s.o_kl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                12 => {
                    s.delay_l = (s.maxdelay_l as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_l != count && s.delay_l % count == 0 {
                                s.delay_l += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_l > s.maxdelay_l {
                        s.delay_l = s.maxdelay_l;
                    }
                    {
                        count = s.alp_ll;
                        while count < 8981 {
                            s.a_ll[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_ll;
                        while count < 8981 {
                            s.o_ll[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                13 => {
                    s.delay_m = (s.maxdelay_m as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_m != count && s.delay_m % count == 0 {
                                s.delay_m += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_m > s.maxdelay_m {
                        s.delay_m = s.maxdelay_m;
                    }
                    {
                        count = s.alp_ml;
                        while count < 8785 {
                            s.a_ml[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_ml;
                        while count < 8785 {
                            s.o_ml[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                14 => {
                    s.delay_n = (s.maxdelay_n as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_n != count && s.delay_n % count == 0 {
                                s.delay_n += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_n > s.maxdelay_n {
                        s.delay_n = s.maxdelay_n;
                    }
                    {
                        count = s.alp_nl;
                        while count < 8461 {
                            s.a_nl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_nl;
                        while count < 8461 {
                            s.o_nl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                15 => {
                    s.delay_o = (s.maxdelay_o as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_o != count && s.delay_o % count == 0 {
                                s.delay_o += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_o > s.maxdelay_o {
                        s.delay_o = s.maxdelay_o;
                    }
                    {
                        count = s.alp_ol;
                        while count < 8309 {
                            s.a_ol[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_ol;
                        while count < 8309 {
                            s.o_ol[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                16 => {
                    s.delay_p = (s.maxdelay_p as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_p != count && s.delay_p % count == 0 {
                                s.delay_p += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_p > s.maxdelay_p {
                        s.delay_p = s.maxdelay_p;
                    }
                    {
                        count = s.alp_pl;
                        while count < 7981 {
                            s.a_pl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_pl;
                        while count < 7981 {
                            s.o_pl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                17 => {
                    s.delay_q = (s.maxdelay_q as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_q != count && s.delay_q % count == 0 {
                                s.delay_q += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_q > s.maxdelay_q {
                        s.delay_q = s.maxdelay_q;
                    }
                    {
                        count = s.alp_ql;
                        while count < 7321 {
                            s.a_ql[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_ql;
                        while count < 7321 {
                            s.o_ql[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                18 => {
                    s.delay_r = (s.maxdelay_r as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_r != count && s.delay_r % count == 0 {
                                s.delay_r += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_r > s.maxdelay_r {
                        s.delay_r = s.maxdelay_r;
                    }
                    {
                        count = s.alp_rl;
                        while count < 6817 {
                            s.a_rl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_rl;
                        while count < 6817 {
                            s.o_rl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                19 => {
                    s.delay_s = (s.maxdelay_s as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_s != count && s.delay_s % count == 0 {
                                s.delay_s += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_s > s.maxdelay_s {
                        s.delay_s = s.maxdelay_s;
                    }
                    {
                        count = s.alp_sl;
                        while count < 6505 {
                            s.a_sl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_sl;
                        while count < 6505 {
                            s.o_sl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                20 => {
                    s.delay_t = (s.maxdelay_t as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_t != count && s.delay_t % count == 0 {
                                s.delay_t += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_t > s.maxdelay_t {
                        s.delay_t = s.maxdelay_t;
                    }
                    {
                        count = s.alp_tl;
                        while count < 6001 {
                            s.a_tl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_tl;
                        while count < 6001 {
                            s.o_tl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                21 => {
                    s.delay_u = (s.maxdelay_u as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_u != count && s.delay_u % count == 0 {
                                s.delay_u += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_u > s.maxdelay_u {
                        s.delay_u = s.maxdelay_u;
                    }
                    {
                        count = s.alp_ul;
                        while count < 5837 {
                            s.a_ul[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_ul;
                        while count < 5837 {
                            s.o_ul[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                22 => {
                    s.delay_v = (s.maxdelay_v as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_v != count && s.delay_v % count == 0 {
                                s.delay_v += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_v > s.maxdelay_v {
                        s.delay_v = s.maxdelay_v;
                    }
                    {
                        count = s.alp_vl;
                        while count < 5501 {
                            s.a_vl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_vl;
                        while count < 5501 {
                            s.o_vl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                23 => {
                    s.delay_w = (s.maxdelay_w as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_w != count && s.delay_w % count == 0 {
                                s.delay_w += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_w > s.maxdelay_w {
                        s.delay_w = s.maxdelay_w;
                    }
                    {
                        count = s.alp_wl;
                        while count < 5009 {
                            s.a_wl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_wl;
                        while count < 5009 {
                            s.o_wl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                24 => {
                    s.delay_x = (s.maxdelay_x as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_x != count && s.delay_x % count == 0 {
                                s.delay_x += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_x > s.maxdelay_x {
                        s.delay_x = s.maxdelay_x;
                    }
                    {
                        count = s.alp_xl;
                        while count < 4849 {
                            s.a_xl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_xl;
                        while count < 4849 {
                            s.o_xl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                25 => {
                    s.delay_y = (s.maxdelay_y as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_y != count && s.delay_y % count == 0 {
                                s.delay_y += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_y > s.maxdelay_y {
                        s.delay_y = s.maxdelay_y;
                    }
                    {
                        count = s.alp_yl;
                        while count < 4295 {
                            s.a_yl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_yl;
                        while count < 4295 {
                            s.o_yl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                26 => {
                    s.delay_z = (s.maxdelay_z as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_z != count && s.delay_z % count == 0 {
                                s.delay_z += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_z > s.maxdelay_z {
                        s.delay_z = s.maxdelay_z;
                    }
                    {
                        count = s.alp_zl;
                        while count < 4179 {
                            s.a_zl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_zl;
                        while count < 4179 {
                            s.o_zl[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        if s.countdown > 0 {
            match s.countdown {
                1 => {
                    s.delay_a = (s.maxdelay_a as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_a != count && s.delay_a % count == 0 {
                                s.delay_a += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_a > s.maxdelay_a {
                        s.delay_a = s.maxdelay_a;
                    }
                    {
                        count = s.alp_ar;
                        while count < 15149 {
                            s.a_ar[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_ar;
                        while count < 15149 {
                            s.o_ar[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                2 => {
                    s.delay_b = (s.maxdelay_b as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_b != count && s.delay_b % count == 0 {
                                s.delay_b += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_b > s.maxdelay_b {
                        s.delay_b = s.maxdelay_b;
                    }
                    {
                        count = s.alp_br;
                        while count < 14617 {
                            s.a_br[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_br;
                        while count < 14617 {
                            s.o_br[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                3 => {
                    s.delay_c = (s.maxdelay_c as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_c != count && s.delay_c % count == 0 {
                                s.delay_c += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_c > s.maxdelay_c {
                        s.delay_c = s.maxdelay_c;
                    }
                    {
                        count = s.alp_cr;
                        while count < 14357 {
                            s.a_cr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_cr;
                        while count < 14357 {
                            s.o_cr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                4 => {
                    s.delay_d = (s.maxdelay_d as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_d != count && s.delay_d % count == 0 {
                                s.delay_d += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_d > s.maxdelay_d {
                        s.delay_d = s.maxdelay_d;
                    }
                    {
                        count = s.alp_dr;
                        while count < 13817 {
                            s.a_dr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_dr;
                        while count < 13817 {
                            s.o_dr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                5 => {
                    s.delay_e = (s.maxdelay_e as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_e != count && s.delay_e % count == 0 {
                                s.delay_e += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_e > s.maxdelay_e {
                        s.delay_e = s.maxdelay_e;
                    }
                    {
                        count = s.alp_er;
                        while count < 13561 {
                            s.a_er[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_er;
                        while count < 13561 {
                            s.o_er[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                6 => {
                    s.delay_f = (s.maxdelay_f as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_f != count && s.delay_f % count == 0 {
                                s.delay_f += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_f > s.maxdelay_f {
                        s.delay_f = s.maxdelay_f;
                    }
                    {
                        count = s.alp_fr;
                        while count < 13045 {
                            s.a_fr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_fr;
                        while count < 13045 {
                            s.o_fr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                7 => {
                    s.delay_g = (s.maxdelay_g as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_g != count && s.delay_g % count == 0 {
                                s.delay_g += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_g > s.maxdelay_g {
                        s.delay_g = s.maxdelay_g;
                    }
                    {
                        count = s.alp_gr;
                        while count < 11965 {
                            s.a_gr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_gr;
                        while count < 11965 {
                            s.o_gr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                8 => {
                    s.delay_h = (s.maxdelay_h as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_h != count && s.delay_h % count == 0 {
                                s.delay_h += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_h > s.maxdelay_h {
                        s.delay_h = s.maxdelay_h;
                    }
                    {
                        count = s.alp_hr;
                        while count < 11129 {
                            s.a_hr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_hr;
                        while count < 11129 {
                            s.o_hr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                9 => {
                    s.delay_i = (s.maxdelay_i as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_i != count && s.delay_i % count == 0 {
                                s.delay_i += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_i > s.maxdelay_i {
                        s.delay_i = s.maxdelay_i;
                    }
                    {
                        count = s.alp_ir;
                        while count < 10597 {
                            s.a_ir[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_ir;
                        while count < 10597 {
                            s.o_ir[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                10 => {
                    s.delay_j = (s.maxdelay_j as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_j != count && s.delay_j % count == 0 {
                                s.delay_j += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_j > s.maxdelay_j {
                        s.delay_j = s.maxdelay_j;
                    }
                    {
                        count = s.alp_jr;
                        while count < 9809 {
                            s.a_jr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_jr;
                        while count < 9809 {
                            s.o_jr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                11 => {
                    s.delay_k = (s.maxdelay_k as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_k != count && s.delay_k % count == 0 {
                                s.delay_k += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_k > s.maxdelay_k {
                        s.delay_k = s.maxdelay_k;
                    }
                    {
                        count = s.alp_kr;
                        while count < 9521 {
                            s.a_kr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_kr;
                        while count < 9521 {
                            s.o_kr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                12 => {
                    s.delay_l = (s.maxdelay_l as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_l != count && s.delay_l % count == 0 {
                                s.delay_l += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_l > s.maxdelay_l {
                        s.delay_l = s.maxdelay_l;
                    }
                    {
                        count = s.alp_lr;
                        while count < 8981 {
                            s.a_lr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_lr;
                        while count < 8981 {
                            s.o_lr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                13 => {
                    s.delay_m = (s.maxdelay_m as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_m != count && s.delay_m % count == 0 {
                                s.delay_m += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_m > s.maxdelay_m {
                        s.delay_m = s.maxdelay_m;
                    }
                    {
                        count = s.alp_mr;
                        while count < 8785 {
                            s.a_mr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_mr;
                        while count < 8785 {
                            s.o_mr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                14 => {
                    s.delay_n = (s.maxdelay_n as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_n != count && s.delay_n % count == 0 {
                                s.delay_n += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_n > s.maxdelay_n {
                        s.delay_n = s.maxdelay_n;
                    }
                    {
                        count = s.alp_nr;
                        while count < 8461 {
                            s.a_nr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_nr;
                        while count < 8461 {
                            s.o_nr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                15 => {
                    s.delay_o = (s.maxdelay_o as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_o != count && s.delay_o % count == 0 {
                                s.delay_o += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_o > s.maxdelay_o {
                        s.delay_o = s.maxdelay_o;
                    }
                    {
                        count = s.alp_or;
                        while count < 8309 {
                            s.a_or[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_or;
                        while count < 8309 {
                            s.o_or[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                16 => {
                    s.delay_p = (s.maxdelay_p as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_p != count && s.delay_p % count == 0 {
                                s.delay_p += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_p > s.maxdelay_p {
                        s.delay_p = s.maxdelay_p;
                    }
                    {
                        count = s.alp_pr;
                        while count < 7981 {
                            s.a_pr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_pr;
                        while count < 7981 {
                            s.o_pr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                17 => {
                    s.delay_q = (s.maxdelay_q as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_q != count && s.delay_q % count == 0 {
                                s.delay_q += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_q > s.maxdelay_q {
                        s.delay_q = s.maxdelay_q;
                    }
                    {
                        count = s.alp_qr;
                        while count < 7321 {
                            s.a_qr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_qr;
                        while count < 7321 {
                            s.o_qr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                18 => {
                    s.delay_r = (s.maxdelay_r as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_r != count && s.delay_r % count == 0 {
                                s.delay_r += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_r > s.maxdelay_r {
                        s.delay_r = s.maxdelay_r;
                    }
                    {
                        count = s.alp_rr;
                        while count < 6817 {
                            s.a_rr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_rr;
                        while count < 6817 {
                            s.o_rr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                19 => {
                    s.delay_s = (s.maxdelay_s as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_s != count && s.delay_s % count == 0 {
                                s.delay_s += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_s > s.maxdelay_s {
                        s.delay_s = s.maxdelay_s;
                    }
                    {
                        count = s.alp_sr;
                        while count < 6505 {
                            s.a_sr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_sr;
                        while count < 6505 {
                            s.o_sr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                20 => {
                    s.delay_t = (s.maxdelay_t as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_t != count && s.delay_t % count == 0 {
                                s.delay_t += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_t > s.maxdelay_t {
                        s.delay_t = s.maxdelay_t;
                    }
                    {
                        count = s.alp_tr;
                        while count < 6001 {
                            s.a_tr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_tr;
                        while count < 6001 {
                            s.o_tr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                21 => {
                    s.delay_u = (s.maxdelay_u as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_u != count && s.delay_u % count == 0 {
                                s.delay_u += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_u > s.maxdelay_u {
                        s.delay_u = s.maxdelay_u;
                    }
                    {
                        count = s.alp_ur;
                        while count < 5837 {
                            s.a_ur[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_ur;
                        while count < 5837 {
                            s.o_ur[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                22 => {
                    s.delay_v = (s.maxdelay_v as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_v != count && s.delay_v % count == 0 {
                                s.delay_v += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_v > s.maxdelay_v {
                        s.delay_v = s.maxdelay_v;
                    }
                    {
                        count = s.alp_vr;
                        while count < 5501 {
                            s.a_vr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_vr;
                        while count < 5501 {
                            s.o_vr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                23 => {
                    s.delay_w = (s.maxdelay_w as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_w != count && s.delay_w % count == 0 {
                                s.delay_w += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_w > s.maxdelay_w {
                        s.delay_w = s.maxdelay_w;
                    }
                    {
                        count = s.alp_wr;
                        while count < 5009 {
                            s.a_wr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_wr;
                        while count < 5009 {
                            s.o_wr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                24 => {
                    s.delay_x = (s.maxdelay_x as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_x != count && s.delay_x % count == 0 {
                                s.delay_x += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_x > s.maxdelay_x {
                        s.delay_x = s.maxdelay_x;
                    }
                    {
                        count = s.alp_xr;
                        while count < 4849 {
                            s.a_xr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_xr;
                        while count < 4849 {
                            s.o_xr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                25 => {
                    s.delay_y = (s.maxdelay_y as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_y != count && s.delay_y % count == 0 {
                                s.delay_y += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_y > s.maxdelay_y {
                        s.delay_y = s.maxdelay_y;
                    }
                    {
                        count = s.alp_yr;
                        while count < 4295 {
                            s.a_yr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_yr;
                        while count < 4295 {
                            s.o_yr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                26 => {
                    s.delay_z = (s.maxdelay_z as f64 * roomsize) as i32;
                    {
                        count = 2;
                        while count <= max {
                            if s.delay_z != count && s.delay_z % count == 0 {
                                s.delay_z += 1;
                                count = 2;
                            }
                            count += 1;
                        }
                    }
                    if s.delay_z > s.maxdelay_z {
                        s.delay_z = s.maxdelay_z;
                    }
                    {
                        count = s.alp_zr;
                        while count < 4179 {
                            s.a_zr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                    {
                        count = s.out_zr;
                        while count < 4179 {
                            s.o_zr[count as usize] = 0.0;
                            count += 1;
                        }
                    }
                }
                _ => {}
            }
            s.countdown -= 1;
        }
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
            s.peak_l -= release;
            if s.peak_l < (input_sample_l * 2.0).abs() {
                s.peak_l = (input_sample_l * 2.0).abs();
            }
            if s.peak_l > 1.0 {
                s.peak_l = 1.0;
            }
            s.peak_r -= release;
            if s.peak_r < (input_sample_r * 2.0).abs() {
                s.peak_r = (input_sample_r * 2.0).abs();
            }
            if s.peak_r > 1.0 {
                s.peak_r = 1.0;
            }
            input_sample_l *= gain;
            bridgerectifier = input_sample_l.abs();
            bridgerectifier = bridgerectifier.sin();
            if input_sample_l > 0.0 {
                input_sample_l = bridgerectifier;
            } else {
                input_sample_l = -bridgerectifier;
            }
            input_sample_r *= gain;
            bridgerectifier = input_sample_r.abs();
            bridgerectifier = bridgerectifier.sin();
            if input_sample_r > 0.0 {
                input_sample_r = bridgerectifier;
            } else {
                input_sample_r = -bridgerectifier;
            }
            match verbtype {
                1 => {
                    allpasstemp = s.alp_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.a_al[allpasstemp as usize] * constallpass;
                    s.a_al[s.alp_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_al -= 1;
                    if s.alp_al < 0 || s.alp_al > s.delay_a {
                        s.alp_al = s.delay_a;
                    }
                    input_sample_l += s.a_al[s.alp_al as usize];
                    s.d_al[3] = s.d_al[2];
                    s.d_al[2] = s.d_al[1];
                    s.d_al[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_al[2] + s.d_al[3]) / 3.0;
                    allpasstemp = s.alp_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.a_bl[allpasstemp as usize] * constallpass;
                    s.a_bl[s.alp_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_bl -= 1;
                    if s.alp_bl < 0 || s.alp_bl > s.delay_b {
                        s.alp_bl = s.delay_b;
                    }
                    input_sample_l += s.a_bl[s.alp_bl as usize];
                    s.d_bl[3] = s.d_bl[2];
                    s.d_bl[2] = s.d_bl[1];
                    s.d_bl[1] = input_sample_l;
                    input_sample_l = (s.d_bl[1] + s.d_bl[2] + s.d_bl[3]) / 3.0;
                    allpasstemp = s.alp_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.a_cl[allpasstemp as usize] * constallpass;
                    s.a_cl[s.alp_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_cl -= 1;
                    if s.alp_cl < 0 || s.alp_cl > s.delay_c {
                        s.alp_cl = s.delay_c;
                    }
                    input_sample_l += s.a_cl[s.alp_cl as usize];
                    s.d_cl[3] = s.d_cl[2];
                    s.d_cl[2] = s.d_cl[1];
                    s.d_cl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_cl[2] + s.d_cl[3]) / 3.0;
                    allpasstemp = s.alp_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.a_dl[allpasstemp as usize] * constallpass;
                    s.a_dl[s.alp_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_dl -= 1;
                    if s.alp_dl < 0 || s.alp_dl > s.delay_d {
                        s.alp_dl = s.delay_d;
                    }
                    input_sample_l += s.a_dl[s.alp_dl as usize];
                    s.d_dl[3] = s.d_dl[2];
                    s.d_dl[2] = s.d_dl[1];
                    s.d_dl[1] = input_sample_l;
                    input_sample_l = (s.d_dl[1] + s.d_dl[2] + s.d_dl[3]) / 3.0;
                    allpasstemp = s.alp_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.a_el[allpasstemp as usize] * constallpass;
                    s.a_el[s.alp_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_el -= 1;
                    if s.alp_el < 0 || s.alp_el > s.delay_e {
                        s.alp_el = s.delay_e;
                    }
                    input_sample_l += s.a_el[s.alp_el as usize];
                    s.d_el[3] = s.d_el[2];
                    s.d_el[2] = s.d_el[1];
                    s.d_el[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_el[2] + s.d_el[3]) / 3.0;
                    allpasstemp = s.alp_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.a_fl[allpasstemp as usize] * constallpass;
                    s.a_fl[s.alp_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_fl -= 1;
                    if s.alp_fl < 0 || s.alp_fl > s.delay_f {
                        s.alp_fl = s.delay_f;
                    }
                    input_sample_l += s.a_fl[s.alp_fl as usize];
                    s.d_fl[3] = s.d_fl[2];
                    s.d_fl[2] = s.d_fl[1];
                    s.d_fl[1] = input_sample_l;
                    input_sample_l = (s.d_fl[1] + s.d_fl[2] + s.d_fl[3]) / 3.0;
                    allpasstemp = s.alp_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.a_gl[allpasstemp as usize] * constallpass;
                    s.a_gl[s.alp_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_gl -= 1;
                    if s.alp_gl < 0 || s.alp_gl > s.delay_g {
                        s.alp_gl = s.delay_g;
                    }
                    input_sample_l += s.a_gl[s.alp_gl as usize];
                    s.d_gl[3] = s.d_gl[2];
                    s.d_gl[2] = s.d_gl[1];
                    s.d_gl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_gl[2] + s.d_gl[3]) / 3.0;
                    allpasstemp = s.alp_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.a_hl[allpasstemp as usize] * constallpass;
                    s.a_hl[s.alp_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_hl -= 1;
                    if s.alp_hl < 0 || s.alp_hl > s.delay_h {
                        s.alp_hl = s.delay_h;
                    }
                    input_sample_l += s.a_hl[s.alp_hl as usize];
                    s.d_hl[3] = s.d_hl[2];
                    s.d_hl[2] = s.d_hl[1];
                    s.d_hl[1] = input_sample_l;
                    input_sample_l = (s.d_hl[1] + s.d_hl[2] + s.d_hl[3]) / 3.0;
                    allpasstemp = s.alp_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.a_il[allpasstemp as usize] * constallpass;
                    s.a_il[s.alp_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_il -= 1;
                    if s.alp_il < 0 || s.alp_il > s.delay_i {
                        s.alp_il = s.delay_i;
                    }
                    input_sample_l += s.a_il[s.alp_il as usize];
                    s.d_il[3] = s.d_il[2];
                    s.d_il[2] = s.d_il[1];
                    s.d_il[1] = input_sample_l;
                    input_sample_l = (s.d_il[1] + s.d_il[2] + s.d_il[3]) / 3.0;
                    allpasstemp = s.alp_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.a_jl[allpasstemp as usize] * constallpass;
                    s.a_jl[s.alp_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_jl -= 1;
                    if s.alp_jl < 0 || s.alp_jl > s.delay_j {
                        s.alp_jl = s.delay_j;
                    }
                    input_sample_l += s.a_jl[s.alp_jl as usize];
                    s.d_jl[3] = s.d_jl[2];
                    s.d_jl[2] = s.d_jl[1];
                    s.d_jl[1] = input_sample_l;
                    input_sample_l = (s.d_jl[1] + s.d_jl[2] + s.d_jl[3]) / 3.0;
                    allpasstemp = s.alp_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.a_kl[allpasstemp as usize] * constallpass;
                    s.a_kl[s.alp_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_kl -= 1;
                    if s.alp_kl < 0 || s.alp_kl > s.delay_k {
                        s.alp_kl = s.delay_k;
                    }
                    input_sample_l += s.a_kl[s.alp_kl as usize];
                    s.d_kl[3] = s.d_kl[2];
                    s.d_kl[2] = s.d_kl[1];
                    s.d_kl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_kl[2] + s.d_kl[3]) / 3.0;
                    allpasstemp = s.alp_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.a_ll[allpasstemp as usize] * constallpass;
                    s.a_ll[s.alp_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ll -= 1;
                    if s.alp_ll < 0 || s.alp_ll > s.delay_l {
                        s.alp_ll = s.delay_l;
                    }
                    input_sample_l += s.a_ll[s.alp_ll as usize];
                    s.d_ll[3] = s.d_ll[2];
                    s.d_ll[2] = s.d_ll[1];
                    s.d_ll[1] = input_sample_l;
                    input_sample_l = (s.d_ll[1] + s.d_ll[2] + s.d_ll[3]) / 3.0;
                    allpasstemp = s.alp_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.a_ml[allpasstemp as usize] * constallpass;
                    s.a_ml[s.alp_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ml -= 1;
                    if s.alp_ml < 0 || s.alp_ml > s.delay_m {
                        s.alp_ml = s.delay_m;
                    }
                    input_sample_l += s.a_ml[s.alp_ml as usize];
                    s.d_ml[3] = s.d_ml[2];
                    s.d_ml[2] = s.d_ml[1];
                    s.d_ml[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ml[2] + s.d_ml[3]) / 3.0;
                    allpasstemp = s.alp_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.a_nl[allpasstemp as usize] * constallpass;
                    s.a_nl[s.alp_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_nl -= 1;
                    if s.alp_nl < 0 || s.alp_nl > s.delay_n {
                        s.alp_nl = s.delay_n;
                    }
                    input_sample_l += s.a_nl[s.alp_nl as usize];
                    s.d_nl[3] = s.d_nl[2];
                    s.d_nl[2] = s.d_nl[1];
                    s.d_nl[1] = input_sample_l;
                    input_sample_l = (s.d_nl[1] + s.d_nl[2] + s.d_nl[3]) / 3.0;
                    allpasstemp = s.alp_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.a_ol[allpasstemp as usize] * constallpass;
                    s.a_ol[s.alp_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ol -= 1;
                    if s.alp_ol < 0 || s.alp_ol > s.delay_o {
                        s.alp_ol = s.delay_o;
                    }
                    input_sample_l += s.a_ol[s.alp_ol as usize];
                    s.d_ol[3] = s.d_ol[2];
                    s.d_ol[2] = s.d_ol[1];
                    s.d_ol[1] = input_sample_l;
                    input_sample_l = (s.d_ol[1] + s.d_ol[2] + s.d_ol[3]) / 3.0;
                    allpasstemp = s.alp_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.a_pl[allpasstemp as usize] * constallpass;
                    s.a_pl[s.alp_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_pl -= 1;
                    if s.alp_pl < 0 || s.alp_pl > s.delay_p {
                        s.alp_pl = s.delay_p;
                    }
                    input_sample_l += s.a_pl[s.alp_pl as usize];
                    s.d_pl[3] = s.d_pl[2];
                    s.d_pl[2] = s.d_pl[1];
                    s.d_pl[1] = input_sample_l;
                    input_sample_l = (s.d_pl[1] + s.d_pl[2] + s.d_pl[3]) / 3.0;
                    allpasstemp = s.alp_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.a_ql[allpasstemp as usize] * constallpass;
                    s.a_ql[s.alp_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ql -= 1;
                    if s.alp_ql < 0 || s.alp_ql > s.delay_q {
                        s.alp_ql = s.delay_q;
                    }
                    input_sample_l += s.a_ql[s.alp_ql as usize];
                    s.d_ql[3] = s.d_ql[2];
                    s.d_ql[2] = s.d_ql[1];
                    s.d_ql[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ql[2] + s.d_ql[3]) / 3.0;
                    allpasstemp = s.alp_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.a_rl[allpasstemp as usize] * constallpass;
                    s.a_rl[s.alp_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_rl -= 1;
                    if s.alp_rl < 0 || s.alp_rl > s.delay_r {
                        s.alp_rl = s.delay_r;
                    }
                    input_sample_l += s.a_rl[s.alp_rl as usize];
                    s.d_rl[3] = s.d_rl[2];
                    s.d_rl[2] = s.d_rl[1];
                    s.d_rl[1] = input_sample_l;
                    input_sample_l = (s.d_rl[1] + s.d_rl[2] + s.d_rl[3]) / 3.0;
                    allpasstemp = s.alp_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.a_sl[allpasstemp as usize] * constallpass;
                    s.a_sl[s.alp_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_sl -= 1;
                    if s.alp_sl < 0 || s.alp_sl > s.delay_s {
                        s.alp_sl = s.delay_s;
                    }
                    input_sample_l += s.a_sl[s.alp_sl as usize];
                    s.d_sl[3] = s.d_sl[2];
                    s.d_sl[2] = s.d_sl[1];
                    s.d_sl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_sl[2] + s.d_sl[3]) / 3.0;
                    allpasstemp = s.alp_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.a_tl[allpasstemp as usize] * constallpass;
                    s.a_tl[s.alp_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_tl -= 1;
                    if s.alp_tl < 0 || s.alp_tl > s.delay_t {
                        s.alp_tl = s.delay_t;
                    }
                    input_sample_l += s.a_tl[s.alp_tl as usize];
                    s.d_tl[3] = s.d_tl[2];
                    s.d_tl[2] = s.d_tl[1];
                    s.d_tl[1] = input_sample_l;
                    input_sample_l = (s.d_tl[1] + s.d_tl[2] + s.d_tl[3]) / 3.0;
                    allpasstemp = s.alp_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.a_ul[allpasstemp as usize] * constallpass;
                    s.a_ul[s.alp_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ul -= 1;
                    if s.alp_ul < 0 || s.alp_ul > s.delay_u {
                        s.alp_ul = s.delay_u;
                    }
                    input_sample_l += s.a_ul[s.alp_ul as usize];
                    s.d_ul[3] = s.d_ul[2];
                    s.d_ul[2] = s.d_ul[1];
                    s.d_ul[1] = input_sample_l;
                    input_sample_l = (s.d_ul[1] + s.d_ul[2] + s.d_ul[3]) / 3.0;
                    allpasstemp = s.alp_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.a_vl[allpasstemp as usize] * constallpass;
                    s.a_vl[s.alp_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_vl -= 1;
                    if s.alp_vl < 0 || s.alp_vl > s.delay_v {
                        s.alp_vl = s.delay_v;
                    }
                    input_sample_l += s.a_vl[s.alp_vl as usize];
                    s.d_vl[3] = s.d_vl[2];
                    s.d_vl[2] = s.d_vl[1];
                    s.d_vl[1] = input_sample_l;
                    input_sample_l = (s.d_vl[1] + s.d_vl[2] + s.d_vl[3]) / 3.0;
                    allpasstemp = s.alp_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.a_wl[allpasstemp as usize] * constallpass;
                    s.a_wl[s.alp_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_wl -= 1;
                    if s.alp_wl < 0 || s.alp_wl > s.delay_w {
                        s.alp_wl = s.delay_w;
                    }
                    input_sample_l += s.a_wl[s.alp_wl as usize];
                    s.d_wl[3] = s.d_wl[2];
                    s.d_wl[2] = s.d_wl[1];
                    s.d_wl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_wl[2] + s.d_wl[3]) / 3.0;
                    allpasstemp = s.alp_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.a_xl[allpasstemp as usize] * constallpass;
                    s.a_xl[s.alp_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_xl -= 1;
                    if s.alp_xl < 0 || s.alp_xl > s.delay_x {
                        s.alp_xl = s.delay_x;
                    }
                    input_sample_l += s.a_xl[s.alp_xl as usize];
                    s.d_xl[3] = s.d_xl[2];
                    s.d_xl[2] = s.d_xl[1];
                    s.d_xl[1] = input_sample_l;
                    input_sample_l = (s.d_xl[1] + s.d_xl[2] + s.d_xl[3]) / 3.0;
                    allpasstemp = s.alp_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.a_yl[allpasstemp as usize] * constallpass;
                    s.a_yl[s.alp_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_yl -= 1;
                    if s.alp_yl < 0 || s.alp_yl > s.delay_y {
                        s.alp_yl = s.delay_y;
                    }
                    input_sample_l += s.a_yl[s.alp_yl as usize];
                    s.d_yl[3] = s.d_yl[2];
                    s.d_yl[2] = s.d_yl[1];
                    s.d_yl[1] = input_sample_l;
                    input_sample_l = (s.d_yl[1] + s.d_yl[2] + s.d_yl[3]) / 3.0;
                    allpasstemp = s.alp_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.a_zl[allpasstemp as usize] * constallpass;
                    s.a_zl[s.alp_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_zl -= 1;
                    if s.alp_zl < 0 || s.alp_zl > s.delay_z {
                        s.alp_zl = s.delay_z;
                    }
                    input_sample_l += s.a_zl[s.alp_zl as usize];
                    s.d_zl[3] = s.d_zl[2];
                    s.d_zl[2] = s.d_zl[1];
                    s.d_zl[1] = input_sample_l;
                    input_sample_l = (s.d_zl[1] + s.d_zl[2] + s.d_zl[3]) / 3.0;
                    allpasstemp = s.out_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.o_al[allpasstemp as usize] * constallpass;
                    s.o_al[s.out_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_al -= 1;
                    if s.out_al < 0 || s.out_al > s.delay_a {
                        s.out_al = s.delay_a;
                    }
                    input_sample_l += s.o_al[s.out_al as usize];
                    s.d_al[6] = s.d_al[5];
                    s.d_al[5] = s.d_al[4];
                    s.d_al[4] = input_sample_l;
                    input_sample_l = (s.d_al[4] + s.d_al[5] + s.d_al[6]) / 3.0;
                    allpasstemp = s.out_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.o_bl[allpasstemp as usize] * constallpass;
                    s.o_bl[s.out_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_bl -= 1;
                    if s.out_bl < 0 || s.out_bl > s.delay_b {
                        s.out_bl = s.delay_b;
                    }
                    input_sample_l += s.o_bl[s.out_bl as usize];
                    s.d_bl[6] = s.d_bl[5];
                    s.d_bl[5] = s.d_bl[4];
                    s.d_bl[4] = input_sample_l;
                    input_sample_l = (s.d_bl[4] + s.d_bl[5] + s.d_bl[6]) / 3.0;
                    allpasstemp = s.out_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.o_cl[allpasstemp as usize] * constallpass;
                    s.o_cl[s.out_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_cl -= 1;
                    if s.out_cl < 0 || s.out_cl > s.delay_c {
                        s.out_cl = s.delay_c;
                    }
                    input_sample_l += s.o_cl[s.out_cl as usize];
                    s.d_cl[6] = s.d_cl[5];
                    s.d_cl[5] = s.d_cl[4];
                    s.d_cl[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_cl[5] + s.d_cl[6]) / 3.0;
                    allpasstemp = s.out_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.o_dl[allpasstemp as usize] * constallpass;
                    s.o_dl[s.out_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_dl -= 1;
                    if s.out_dl < 0 || s.out_dl > s.delay_d {
                        s.out_dl = s.delay_d;
                    }
                    input_sample_l += s.o_dl[s.out_dl as usize];
                    s.d_dl[6] = s.d_dl[5];
                    s.d_dl[5] = s.d_dl[4];
                    s.d_dl[4] = input_sample_l;
                    input_sample_l = (s.d_dl[4] + s.d_dl[5] + s.d_dl[6]) / 3.0;
                    allpasstemp = s.out_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.o_el[allpasstemp as usize] * constallpass;
                    s.o_el[s.out_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_el -= 1;
                    if s.out_el < 0 || s.out_el > s.delay_e {
                        s.out_el = s.delay_e;
                    }
                    input_sample_l += s.o_el[s.out_el as usize];
                    s.d_el[6] = s.d_el[5];
                    s.d_el[5] = s.d_el[4];
                    s.d_el[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_el[5] + s.d_el[6]) / 3.0;
                    allpasstemp = s.out_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.o_fl[allpasstemp as usize] * constallpass;
                    s.o_fl[s.out_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_fl -= 1;
                    if s.out_fl < 0 || s.out_fl > s.delay_f {
                        s.out_fl = s.delay_f;
                    }
                    input_sample_l += s.o_fl[s.out_fl as usize];
                    s.d_fl[6] = s.d_fl[5];
                    s.d_fl[5] = s.d_fl[4];
                    s.d_fl[4] = input_sample_l;
                    input_sample_l = (s.d_fl[4] + s.d_fl[5] + s.d_fl[6]) / 3.0;
                    allpasstemp = s.out_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.o_gl[allpasstemp as usize] * constallpass;
                    s.o_gl[s.out_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_gl -= 1;
                    if s.out_gl < 0 || s.out_gl > s.delay_g {
                        s.out_gl = s.delay_g;
                    }
                    input_sample_l += s.o_gl[s.out_gl as usize];
                    s.d_gl[6] = s.d_gl[5];
                    s.d_gl[5] = s.d_gl[4];
                    s.d_gl[4] = input_sample_l;
                    input_sample_l = (s.d_gl[4] + s.d_gl[5] + s.d_gl[6]) / 3.0;
                    allpasstemp = s.out_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.o_hl[allpasstemp as usize] * constallpass;
                    s.o_hl[s.out_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_hl -= 1;
                    if s.out_hl < 0 || s.out_hl > s.delay_h {
                        s.out_hl = s.delay_h;
                    }
                    input_sample_l += s.o_hl[s.out_hl as usize];
                    s.d_hl[6] = s.d_hl[5];
                    s.d_hl[5] = s.d_hl[4];
                    s.d_hl[4] = input_sample_l;
                    input_sample_l = (s.d_hl[4] + s.d_hl[5] + s.d_hl[6]) / 3.0;
                    allpasstemp = s.out_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.o_il[allpasstemp as usize] * constallpass;
                    s.o_il[s.out_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_il -= 1;
                    if s.out_il < 0 || s.out_il > s.delay_i {
                        s.out_il = s.delay_i;
                    }
                    input_sample_l += s.o_il[s.out_il as usize];
                    s.d_il[6] = s.d_il[5];
                    s.d_il[5] = s.d_il[4];
                    s.d_il[4] = input_sample_l;
                    input_sample_l = (s.d_il[4] + s.d_il[5] + s.d_il[6]) / 3.0;
                    allpasstemp = s.out_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.o_jl[allpasstemp as usize] * constallpass;
                    s.o_jl[s.out_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_jl -= 1;
                    if s.out_jl < 0 || s.out_jl > s.delay_j {
                        s.out_jl = s.delay_j;
                    }
                    input_sample_l += s.o_jl[s.out_jl as usize];
                    s.d_jl[6] = s.d_jl[5];
                    s.d_jl[5] = s.d_jl[4];
                    s.d_jl[4] = input_sample_l;
                    input_sample_l = (s.d_jl[4] + s.d_jl[5] + s.d_jl[6]) / 3.0;
                    allpasstemp = s.out_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.o_kl[allpasstemp as usize] * constallpass;
                    s.o_kl[s.out_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_kl -= 1;
                    if s.out_kl < 0 || s.out_kl > s.delay_k {
                        s.out_kl = s.delay_k;
                    }
                    input_sample_l += s.o_kl[s.out_kl as usize];
                    s.d_kl[6] = s.d_kl[5];
                    s.d_kl[5] = s.d_kl[4];
                    s.d_kl[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_kl[5] + s.d_kl[6]) / 3.0;
                    allpasstemp = s.out_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.o_ll[allpasstemp as usize] * constallpass;
                    s.o_ll[s.out_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ll -= 1;
                    if s.out_ll < 0 || s.out_ll > s.delay_l {
                        s.out_ll = s.delay_l;
                    }
                    input_sample_l += s.o_ll[s.out_ll as usize];
                    s.d_ll[6] = s.d_ll[5];
                    s.d_ll[5] = s.d_ll[4];
                    s.d_ll[4] = input_sample_l;
                    input_sample_l = (s.d_ll[4] + s.d_ll[5] + s.d_ll[6]) / 3.0;
                    allpasstemp = s.out_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.o_ml[allpasstemp as usize] * constallpass;
                    s.o_ml[s.out_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ml -= 1;
                    if s.out_ml < 0 || s.out_ml > s.delay_m {
                        s.out_ml = s.delay_m;
                    }
                    input_sample_l += s.o_ml[s.out_ml as usize];
                    s.d_ml[6] = s.d_ml[5];
                    s.d_ml[5] = s.d_ml[4];
                    s.d_ml[4] = input_sample_l;
                    input_sample_l = (s.d_ml[4] + s.d_ml[5] + s.d_ml[6]) / 3.0;
                    allpasstemp = s.out_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.o_nl[allpasstemp as usize] * constallpass;
                    s.o_nl[s.out_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_nl -= 1;
                    if s.out_nl < 0 || s.out_nl > s.delay_n {
                        s.out_nl = s.delay_n;
                    }
                    input_sample_l += s.o_nl[s.out_nl as usize];
                    s.d_nl[6] = s.d_nl[5];
                    s.d_nl[5] = s.d_nl[4];
                    s.d_nl[4] = input_sample_l;
                    input_sample_l = (s.d_nl[4] + s.d_nl[5] + s.d_nl[6]) / 3.0;
                    allpasstemp = s.out_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.o_ol[allpasstemp as usize] * constallpass;
                    s.o_ol[s.out_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ol -= 1;
                    if s.out_ol < 0 || s.out_ol > s.delay_o {
                        s.out_ol = s.delay_o;
                    }
                    input_sample_l += s.o_ol[s.out_ol as usize];
                    s.d_ol[6] = s.d_ol[5];
                    s.d_ol[5] = s.d_ol[4];
                    s.d_ol[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ol[5] + s.d_ol[6]) / 3.0;
                    allpasstemp = s.out_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.o_pl[allpasstemp as usize] * constallpass;
                    s.o_pl[s.out_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_pl -= 1;
                    if s.out_pl < 0 || s.out_pl > s.delay_p {
                        s.out_pl = s.delay_p;
                    }
                    input_sample_l += s.o_pl[s.out_pl as usize];
                    s.d_pl[6] = s.d_pl[5];
                    s.d_pl[5] = s.d_pl[4];
                    s.d_pl[4] = input_sample_l;
                    input_sample_l = (s.d_pl[4] + s.d_pl[5] + s.d_pl[6]) / 3.0;
                    allpasstemp = s.out_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.o_ql[allpasstemp as usize] * constallpass;
                    s.o_ql[s.out_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ql -= 1;
                    if s.out_ql < 0 || s.out_ql > s.delay_q {
                        s.out_ql = s.delay_q;
                    }
                    input_sample_l += s.o_ql[s.out_ql as usize];
                    s.d_ql[6] = s.d_ql[5];
                    s.d_ql[5] = s.d_ql[4];
                    s.d_ql[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ql[5] + s.d_ql[6]) / 3.0;
                    allpasstemp = s.out_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.o_rl[allpasstemp as usize] * constallpass;
                    s.o_rl[s.out_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_rl -= 1;
                    if s.out_rl < 0 || s.out_rl > s.delay_r {
                        s.out_rl = s.delay_r;
                    }
                    input_sample_l += s.o_rl[s.out_rl as usize];
                    s.d_rl[6] = s.d_rl[5];
                    s.d_rl[5] = s.d_rl[4];
                    s.d_rl[4] = input_sample_l;
                    input_sample_l = (s.d_rl[4] + s.d_rl[5] + s.d_rl[6]) / 3.0;
                    allpasstemp = s.out_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.o_sl[allpasstemp as usize] * constallpass;
                    s.o_sl[s.out_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_sl -= 1;
                    if s.out_sl < 0 || s.out_sl > s.delay_s {
                        s.out_sl = s.delay_s;
                    }
                    input_sample_l += s.o_sl[s.out_sl as usize];
                    s.d_sl[6] = s.d_sl[5];
                    s.d_sl[5] = s.d_sl[4];
                    s.d_sl[4] = input_sample_l;
                    input_sample_l = (s.d_sl[4] + s.d_sl[5] + s.d_sl[6]) / 3.0;
                    allpasstemp = s.out_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.o_tl[allpasstemp as usize] * constallpass;
                    s.o_tl[s.out_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_tl -= 1;
                    if s.out_tl < 0 || s.out_tl > s.delay_t {
                        s.out_tl = s.delay_t;
                    }
                    input_sample_l += s.o_tl[s.out_tl as usize];
                    s.d_tl[6] = s.d_tl[5];
                    s.d_tl[5] = s.d_tl[4];
                    s.d_tl[4] = input_sample_l;
                    input_sample_l = (s.d_tl[4] + s.d_tl[5] + s.d_tl[6]) / 3.0;
                    allpasstemp = s.out_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.o_ul[allpasstemp as usize] * constallpass;
                    s.o_ul[s.out_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ul -= 1;
                    if s.out_ul < 0 || s.out_ul > s.delay_u {
                        s.out_ul = s.delay_u;
                    }
                    input_sample_l += s.o_ul[s.out_ul as usize];
                    s.d_ul[6] = s.d_ul[5];
                    s.d_ul[5] = s.d_ul[4];
                    s.d_ul[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ul[5] + s.d_ul[6]) / 3.0;
                    allpasstemp = s.out_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.o_vl[allpasstemp as usize] * constallpass;
                    s.o_vl[s.out_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_vl -= 1;
                    if s.out_vl < 0 || s.out_vl > s.delay_v {
                        s.out_vl = s.delay_v;
                    }
                    input_sample_l += s.o_vl[s.out_vl as usize];
                    s.d_vl[6] = s.d_vl[5];
                    s.d_vl[5] = s.d_vl[4];
                    s.d_vl[4] = input_sample_l;
                    input_sample_l = (s.d_vl[4] + s.d_vl[5] + s.d_vl[6]) / 3.0;
                    allpasstemp = s.out_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.o_wl[allpasstemp as usize] * constallpass;
                    s.o_wl[s.out_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_wl -= 1;
                    if s.out_wl < 0 || s.out_wl > s.delay_w {
                        s.out_wl = s.delay_w;
                    }
                    input_sample_l += s.o_wl[s.out_wl as usize];
                    s.d_wl[6] = s.d_wl[5];
                    s.d_wl[5] = s.d_wl[4];
                    s.d_wl[4] = input_sample_l;
                    input_sample_l = (s.d_wl[4] + s.d_wl[5] + s.d_wl[6]) / 3.0;
                    allpasstemp = s.out_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.o_xl[allpasstemp as usize] * constallpass;
                    s.o_xl[s.out_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_xl -= 1;
                    if s.out_xl < 0 || s.out_xl > s.delay_x {
                        s.out_xl = s.delay_x;
                    }
                    input_sample_l += s.o_xl[s.out_xl as usize];
                    s.d_xl[6] = s.d_xl[5];
                    s.d_xl[5] = s.d_xl[4];
                    s.d_xl[4] = input_sample_l;
                    input_sample_l = (s.d_xl[4] + s.d_xl[5] + s.d_xl[6]) / 3.0;
                    allpasstemp = s.out_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.o_yl[allpasstemp as usize] * constallpass;
                    s.o_yl[s.out_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_yl -= 1;
                    if s.out_yl < 0 || s.out_yl > s.delay_y {
                        s.out_yl = s.delay_y;
                    }
                    input_sample_l += s.o_yl[s.out_yl as usize];
                    s.d_yl[6] = s.d_yl[5];
                    s.d_yl[5] = s.d_yl[4];
                    s.d_yl[4] = input_sample_l;
                    input_sample_l = (s.d_yl[4] + s.d_yl[5] + s.d_yl[6]) / 3.0;
                    allpasstemp = s.out_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.o_zl[allpasstemp as usize] * constallpass;
                    s.o_zl[s.out_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_zl -= 1;
                    if s.out_zl < 0 || s.out_zl > s.delay_z {
                        s.out_zl = s.delay_z;
                    }
                    input_sample_l += s.o_zl[s.out_zl as usize];
                    s.d_zl[6] = s.d_zl[5];
                    s.d_zl[5] = s.d_zl[4];
                    s.d_zl[4] = input_sample_l;
                    input_sample_l = s.d_zl[4] + s.d_zl[5] + s.d_zl[6];
                }
                2 => {
                    allpasstemp = s.alp_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.a_al[allpasstemp as usize] * constallpass;
                    s.a_al[s.alp_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_al -= 1;
                    if s.alp_al < 0 || s.alp_al > s.delay_a {
                        s.alp_al = s.delay_a;
                    }
                    input_sample_l += s.a_al[s.alp_al as usize];
                    s.d_al[3] = s.d_al[2];
                    s.d_al[2] = s.d_al[1];
                    s.d_al[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_al[2] + s.d_al[3]) / 3.0;
                    allpasstemp = s.alp_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.a_bl[allpasstemp as usize] * constallpass;
                    s.a_bl[s.alp_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_bl -= 1;
                    if s.alp_bl < 0 || s.alp_bl > s.delay_b {
                        s.alp_bl = s.delay_b;
                    }
                    input_sample_l += s.a_bl[s.alp_bl as usize];
                    s.d_bl[3] = s.d_bl[2];
                    s.d_bl[2] = s.d_bl[1];
                    s.d_bl[1] = input_sample_l;
                    input_sample_l = (s.d_bl[1] + s.d_bl[2] + s.d_bl[3]) / 3.0;
                    allpasstemp = s.alp_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.a_cl[allpasstemp as usize] * constallpass;
                    s.a_cl[s.alp_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_cl -= 1;
                    if s.alp_cl < 0 || s.alp_cl > s.delay_c {
                        s.alp_cl = s.delay_c;
                    }
                    input_sample_l += s.a_cl[s.alp_cl as usize];
                    s.d_cl[3] = s.d_cl[2];
                    s.d_cl[2] = s.d_cl[1];
                    s.d_cl[1] = input_sample_l;
                    input_sample_l = (s.d_cl[1] + s.d_cl[2] + s.d_cl[3]) / 3.0;
                    allpasstemp = s.alp_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.a_dl[allpasstemp as usize] * constallpass;
                    s.a_dl[s.alp_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_dl -= 1;
                    if s.alp_dl < 0 || s.alp_dl > s.delay_d {
                        s.alp_dl = s.delay_d;
                    }
                    input_sample_l += s.a_dl[s.alp_dl as usize];
                    s.d_dl[3] = s.d_dl[2];
                    s.d_dl[2] = s.d_dl[1];
                    s.d_dl[1] = input_sample_l;
                    input_sample_l = (s.d_dl[1] + s.d_dl[2] + s.d_dl[3]) / 3.0;
                    allpasstemp = s.alp_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.a_el[allpasstemp as usize] * constallpass;
                    s.a_el[s.alp_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_el -= 1;
                    if s.alp_el < 0 || s.alp_el > s.delay_e {
                        s.alp_el = s.delay_e;
                    }
                    input_sample_l += s.a_el[s.alp_el as usize];
                    s.d_el[3] = s.d_el[2];
                    s.d_el[2] = s.d_el[1];
                    s.d_el[1] = input_sample_l;
                    input_sample_l = (s.d_el[1] + s.d_el[2] + s.d_el[3]) / 3.0;
                    allpasstemp = s.alp_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.a_fl[allpasstemp as usize] * constallpass;
                    s.a_fl[s.alp_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_fl -= 1;
                    if s.alp_fl < 0 || s.alp_fl > s.delay_f {
                        s.alp_fl = s.delay_f;
                    }
                    input_sample_l += s.a_fl[s.alp_fl as usize];
                    s.d_fl[3] = s.d_fl[2];
                    s.d_fl[2] = s.d_fl[1];
                    s.d_fl[1] = input_sample_l;
                    input_sample_l = (s.d_fl[1] + s.d_fl[2] + s.d_fl[3]) / 3.0;
                    allpasstemp = s.alp_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.a_gl[allpasstemp as usize] * constallpass;
                    s.a_gl[s.alp_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_gl -= 1;
                    if s.alp_gl < 0 || s.alp_gl > s.delay_g {
                        s.alp_gl = s.delay_g;
                    }
                    input_sample_l += s.a_gl[s.alp_gl as usize];
                    s.d_gl[3] = s.d_gl[2];
                    s.d_gl[2] = s.d_gl[1];
                    s.d_gl[1] = input_sample_l;
                    input_sample_l = (s.d_gl[1] + s.d_gl[2] + s.d_gl[3]) / 3.0;
                    allpasstemp = s.alp_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.a_hl[allpasstemp as usize] * constallpass;
                    s.a_hl[s.alp_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_hl -= 1;
                    if s.alp_hl < 0 || s.alp_hl > s.delay_h {
                        s.alp_hl = s.delay_h;
                    }
                    input_sample_l += s.a_hl[s.alp_hl as usize];
                    s.d_hl[3] = s.d_hl[2];
                    s.d_hl[2] = s.d_hl[1];
                    s.d_hl[1] = input_sample_l;
                    input_sample_l = (s.d_hl[1] + s.d_hl[2] + s.d_hl[3]) / 3.0;
                    allpasstemp = s.alp_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.a_il[allpasstemp as usize] * constallpass;
                    s.a_il[s.alp_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_il -= 1;
                    if s.alp_il < 0 || s.alp_il > s.delay_i {
                        s.alp_il = s.delay_i;
                    }
                    input_sample_l += s.a_il[s.alp_il as usize];
                    s.d_il[3] = s.d_il[2];
                    s.d_il[2] = s.d_il[1];
                    s.d_il[1] = input_sample_l;
                    input_sample_l = (s.d_il[1] + s.d_il[2] + s.d_il[3]) / 3.0;
                    allpasstemp = s.alp_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.a_jl[allpasstemp as usize] * constallpass;
                    s.a_jl[s.alp_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_jl -= 1;
                    if s.alp_jl < 0 || s.alp_jl > s.delay_j {
                        s.alp_jl = s.delay_j;
                    }
                    input_sample_l += s.a_jl[s.alp_jl as usize];
                    s.d_jl[3] = s.d_jl[2];
                    s.d_jl[2] = s.d_jl[1];
                    s.d_jl[1] = input_sample_l;
                    input_sample_l = (s.d_jl[1] + s.d_jl[2] + s.d_jl[3]) / 3.0;
                    allpasstemp = s.alp_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.a_kl[allpasstemp as usize] * constallpass;
                    s.a_kl[s.alp_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_kl -= 1;
                    if s.alp_kl < 0 || s.alp_kl > s.delay_k {
                        s.alp_kl = s.delay_k;
                    }
                    input_sample_l += s.a_kl[s.alp_kl as usize];
                    s.d_kl[3] = s.d_kl[2];
                    s.d_kl[2] = s.d_kl[1];
                    s.d_kl[1] = input_sample_l;
                    input_sample_l = (s.d_kl[1] + s.d_kl[2] + s.d_kl[3]) / 3.0;
                    allpasstemp = s.alp_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.a_ll[allpasstemp as usize] * constallpass;
                    s.a_ll[s.alp_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ll -= 1;
                    if s.alp_ll < 0 || s.alp_ll > s.delay_l {
                        s.alp_ll = s.delay_l;
                    }
                    input_sample_l += s.a_ll[s.alp_ll as usize];
                    s.d_ll[3] = s.d_ll[2];
                    s.d_ll[2] = s.d_ll[1];
                    s.d_ll[1] = input_sample_l;
                    input_sample_l = (s.d_ll[1] + s.d_ll[2] + s.d_ll[3]) / 3.0;
                    allpasstemp = s.alp_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.a_ml[allpasstemp as usize] * constallpass;
                    s.a_ml[s.alp_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ml -= 1;
                    if s.alp_ml < 0 || s.alp_ml > s.delay_m {
                        s.alp_ml = s.delay_m;
                    }
                    input_sample_l += s.a_ml[s.alp_ml as usize];
                    s.d_ml[3] = s.d_ml[2];
                    s.d_ml[2] = s.d_ml[1];
                    s.d_ml[1] = input_sample_l;
                    input_sample_l = (s.d_ml[1] + s.d_ml[2] + s.d_ml[3]) / 3.0;
                    allpasstemp = s.alp_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.a_nl[allpasstemp as usize] * constallpass;
                    s.a_nl[s.alp_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_nl -= 1;
                    if s.alp_nl < 0 || s.alp_nl > s.delay_n {
                        s.alp_nl = s.delay_n;
                    }
                    input_sample_l += s.a_nl[s.alp_nl as usize];
                    s.d_nl[3] = s.d_nl[2];
                    s.d_nl[2] = s.d_nl[1];
                    s.d_nl[1] = input_sample_l;
                    input_sample_l = (s.d_nl[1] + s.d_nl[2] + s.d_nl[3]) / 3.0;
                    allpasstemp = s.alp_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.a_ol[allpasstemp as usize] * constallpass;
                    s.a_ol[s.alp_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ol -= 1;
                    if s.alp_ol < 0 || s.alp_ol > s.delay_o {
                        s.alp_ol = s.delay_o;
                    }
                    input_sample_l += s.a_ol[s.alp_ol as usize];
                    s.d_ol[3] = s.d_ol[2];
                    s.d_ol[2] = s.d_ol[1];
                    s.d_ol[1] = input_sample_l;
                    input_sample_l = (s.d_ol[1] + s.d_ol[2] + s.d_ol[3]) / 3.0;
                    allpasstemp = s.alp_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.a_pl[allpasstemp as usize] * constallpass;
                    s.a_pl[s.alp_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_pl -= 1;
                    if s.alp_pl < 0 || s.alp_pl > s.delay_p {
                        s.alp_pl = s.delay_p;
                    }
                    input_sample_l += s.a_pl[s.alp_pl as usize];
                    s.d_pl[3] = s.d_pl[2];
                    s.d_pl[2] = s.d_pl[1];
                    s.d_pl[1] = input_sample_l;
                    input_sample_l = (s.d_pl[1] + s.d_pl[2] + s.d_pl[3]) / 3.0;
                    allpasstemp = s.alp_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.a_ql[allpasstemp as usize] * constallpass;
                    s.a_ql[s.alp_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ql -= 1;
                    if s.alp_ql < 0 || s.alp_ql > s.delay_q {
                        s.alp_ql = s.delay_q;
                    }
                    input_sample_l += s.a_ql[s.alp_ql as usize];
                    s.d_ql[3] = s.d_ql[2];
                    s.d_ql[2] = s.d_ql[1];
                    s.d_ql[1] = input_sample_l;
                    input_sample_l = (s.d_ql[1] + s.d_ql[2] + s.d_ql[3]) / 3.0;
                    allpasstemp = s.alp_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.a_rl[allpasstemp as usize] * constallpass;
                    s.a_rl[s.alp_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_rl -= 1;
                    if s.alp_rl < 0 || s.alp_rl > s.delay_r {
                        s.alp_rl = s.delay_r;
                    }
                    input_sample_l += s.a_rl[s.alp_rl as usize];
                    s.d_rl[3] = s.d_rl[2];
                    s.d_rl[2] = s.d_rl[1];
                    s.d_rl[1] = input_sample_l;
                    input_sample_l = (s.d_rl[1] + s.d_rl[2] + s.d_rl[3]) / 3.0;
                    allpasstemp = s.alp_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.a_sl[allpasstemp as usize] * constallpass;
                    s.a_sl[s.alp_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_sl -= 1;
                    if s.alp_sl < 0 || s.alp_sl > s.delay_s {
                        s.alp_sl = s.delay_s;
                    }
                    input_sample_l += s.a_sl[s.alp_sl as usize];
                    s.d_sl[3] = s.d_sl[2];
                    s.d_sl[2] = s.d_sl[1];
                    s.d_sl[1] = input_sample_l;
                    input_sample_l = (s.d_sl[1] + s.d_sl[2] + s.d_sl[3]) / 3.0;
                    allpasstemp = s.alp_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.a_tl[allpasstemp as usize] * constallpass;
                    s.a_tl[s.alp_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_tl -= 1;
                    if s.alp_tl < 0 || s.alp_tl > s.delay_t {
                        s.alp_tl = s.delay_t;
                    }
                    input_sample_l += s.a_tl[s.alp_tl as usize];
                    s.d_tl[3] = s.d_tl[2];
                    s.d_tl[2] = s.d_tl[1];
                    s.d_tl[1] = input_sample_l;
                    input_sample_l = (s.d_tl[1] + s.d_tl[2] + s.d_tl[3]) / 3.0;
                    allpasstemp = s.alp_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.a_ul[allpasstemp as usize] * constallpass;
                    s.a_ul[s.alp_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ul -= 1;
                    if s.alp_ul < 0 || s.alp_ul > s.delay_u {
                        s.alp_ul = s.delay_u;
                    }
                    input_sample_l += s.a_ul[s.alp_ul as usize];
                    s.d_ul[3] = s.d_ul[2];
                    s.d_ul[2] = s.d_ul[1];
                    s.d_ul[1] = input_sample_l;
                    input_sample_l = (s.d_ul[1] + s.d_ul[2] + s.d_ul[3]) / 3.0;
                    allpasstemp = s.alp_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.a_vl[allpasstemp as usize] * constallpass;
                    s.a_vl[s.alp_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_vl -= 1;
                    if s.alp_vl < 0 || s.alp_vl > s.delay_v {
                        s.alp_vl = s.delay_v;
                    }
                    input_sample_l += s.a_vl[s.alp_vl as usize];
                    s.d_vl[3] = s.d_vl[2];
                    s.d_vl[2] = s.d_vl[1];
                    s.d_vl[1] = input_sample_l;
                    input_sample_l = (s.d_vl[1] + s.d_vl[2] + s.d_vl[3]) / 3.0;
                    allpasstemp = s.alp_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.a_wl[allpasstemp as usize] * constallpass;
                    s.a_wl[s.alp_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_wl -= 1;
                    if s.alp_wl < 0 || s.alp_wl > s.delay_w {
                        s.alp_wl = s.delay_w;
                    }
                    input_sample_l += s.a_wl[s.alp_wl as usize];
                    s.d_wl[3] = s.d_wl[2];
                    s.d_wl[2] = s.d_wl[1];
                    s.d_wl[1] = input_sample_l;
                    input_sample_l = (s.d_wl[1] + s.d_wl[2] + s.d_wl[3]) / 3.0;
                    allpasstemp = s.alp_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.a_xl[allpasstemp as usize] * constallpass;
                    s.a_xl[s.alp_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_xl -= 1;
                    if s.alp_xl < 0 || s.alp_xl > s.delay_x {
                        s.alp_xl = s.delay_x;
                    }
                    input_sample_l += s.a_xl[s.alp_xl as usize];
                    s.d_xl[3] = s.d_xl[2];
                    s.d_xl[2] = s.d_xl[1];
                    s.d_xl[1] = input_sample_l;
                    input_sample_l = (s.d_xl[1] + s.d_xl[2] + s.d_xl[3]) / 3.0;
                    allpasstemp = s.alp_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.a_yl[allpasstemp as usize] * constallpass;
                    s.a_yl[s.alp_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_yl -= 1;
                    if s.alp_yl < 0 || s.alp_yl > s.delay_y {
                        s.alp_yl = s.delay_y;
                    }
                    input_sample_l += s.a_yl[s.alp_yl as usize];
                    s.d_yl[3] = s.d_yl[2];
                    s.d_yl[2] = s.d_yl[1];
                    s.d_yl[1] = input_sample_l;
                    input_sample_l = (s.d_yl[1] + s.d_yl[2] + s.d_yl[3]) / 3.0;
                    allpasstemp = s.alp_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.a_zl[allpasstemp as usize] * constallpass;
                    s.a_zl[s.alp_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_zl -= 1;
                    if s.alp_zl < 0 || s.alp_zl > s.delay_z {
                        s.alp_zl = s.delay_z;
                    }
                    input_sample_l += s.a_zl[s.alp_zl as usize];
                    s.d_zl[3] = s.d_zl[2];
                    s.d_zl[2] = s.d_zl[1];
                    s.d_zl[1] = input_sample_l;
                    input_sample_l = (s.d_zl[1] + s.d_zl[2] + s.d_zl[3]) / 3.0;
                    allpasstemp = s.out_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.o_al[allpasstemp as usize] * constallpass;
                    s.o_al[s.out_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_al -= 1;
                    if s.out_al < 0 || s.out_al > s.delay_a {
                        s.out_al = s.delay_a;
                    }
                    input_sample_l += s.o_al[s.out_al as usize];
                    s.d_al[6] = s.d_al[5];
                    s.d_al[5] = s.d_al[4];
                    s.d_al[4] = input_sample_l;
                    input_sample_l = (s.d_yl[1] + s.d_al[5] + s.d_al[6]) / 3.0;
                    allpasstemp = s.out_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.o_bl[allpasstemp as usize] * constallpass;
                    s.o_bl[s.out_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_bl -= 1;
                    if s.out_bl < 0 || s.out_bl > s.delay_b {
                        s.out_bl = s.delay_b;
                    }
                    input_sample_l += s.o_bl[s.out_bl as usize];
                    s.d_bl[6] = s.d_bl[5];
                    s.d_bl[5] = s.d_bl[4];
                    s.d_bl[4] = input_sample_l;
                    input_sample_l = (s.d_xl[1] + s.d_bl[5] + s.d_bl[6]) / 3.0;
                    allpasstemp = s.out_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.o_cl[allpasstemp as usize] * constallpass;
                    s.o_cl[s.out_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_cl -= 1;
                    if s.out_cl < 0 || s.out_cl > s.delay_c {
                        s.out_cl = s.delay_c;
                    }
                    input_sample_l += s.o_cl[s.out_cl as usize];
                    s.d_cl[6] = s.d_cl[5];
                    s.d_cl[5] = s.d_cl[4];
                    s.d_cl[4] = input_sample_l;
                    input_sample_l = (s.d_wl[1] + s.d_cl[5] + s.d_cl[6]) / 3.0;
                    allpasstemp = s.out_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.o_dl[allpasstemp as usize] * constallpass;
                    s.o_dl[s.out_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_dl -= 1;
                    if s.out_dl < 0 || s.out_dl > s.delay_d {
                        s.out_dl = s.delay_d;
                    }
                    input_sample_l += s.o_dl[s.out_dl as usize];
                    s.d_dl[6] = s.d_dl[5];
                    s.d_dl[5] = s.d_dl[4];
                    s.d_dl[4] = input_sample_l;
                    input_sample_l = (s.d_vl[1] + s.d_dl[5] + s.d_dl[6]) / 3.0;
                    allpasstemp = s.out_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.o_el[allpasstemp as usize] * constallpass;
                    s.o_el[s.out_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_el -= 1;
                    if s.out_el < 0 || s.out_el > s.delay_e {
                        s.out_el = s.delay_e;
                    }
                    input_sample_l += s.o_el[s.out_el as usize];
                    s.d_el[6] = s.d_el[5];
                    s.d_el[5] = s.d_el[4];
                    s.d_el[4] = input_sample_l;
                    input_sample_l = (s.d_ul[1] + s.d_el[5] + s.d_el[6]) / 3.0;
                    allpasstemp = s.out_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.o_fl[allpasstemp as usize] * constallpass;
                    s.o_fl[s.out_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_fl -= 1;
                    if s.out_fl < 0 || s.out_fl > s.delay_f {
                        s.out_fl = s.delay_f;
                    }
                    input_sample_l += s.o_fl[s.out_fl as usize];
                    s.d_fl[6] = s.d_fl[5];
                    s.d_fl[5] = s.d_fl[4];
                    s.d_fl[4] = input_sample_l;
                    input_sample_l = (s.d_tl[1] + s.d_fl[5] + s.d_fl[6]) / 3.0;
                    allpasstemp = s.out_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.o_gl[allpasstemp as usize] * constallpass;
                    s.o_gl[s.out_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_gl -= 1;
                    if s.out_gl < 0 || s.out_gl > s.delay_g {
                        s.out_gl = s.delay_g;
                    }
                    input_sample_l += s.o_gl[s.out_gl as usize];
                    s.d_gl[6] = s.d_gl[5];
                    s.d_gl[5] = s.d_gl[4];
                    s.d_gl[4] = input_sample_l;
                    input_sample_l = (s.d_sl[1] + s.d_gl[5] + s.d_gl[6]) / 3.0;
                    allpasstemp = s.out_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.o_hl[allpasstemp as usize] * constallpass;
                    s.o_hl[s.out_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_hl -= 1;
                    if s.out_hl < 0 || s.out_hl > s.delay_h {
                        s.out_hl = s.delay_h;
                    }
                    input_sample_l += s.o_hl[s.out_hl as usize];
                    s.d_hl[6] = s.d_hl[5];
                    s.d_hl[5] = s.d_hl[4];
                    s.d_hl[4] = input_sample_l;
                    input_sample_l = (s.d_rl[1] + s.d_hl[5] + s.d_hl[6]) / 3.0;
                    allpasstemp = s.out_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.o_il[allpasstemp as usize] * constallpass;
                    s.o_il[s.out_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_il -= 1;
                    if s.out_il < 0 || s.out_il > s.delay_i {
                        s.out_il = s.delay_i;
                    }
                    input_sample_l += s.o_il[s.out_il as usize];
                    s.d_il[6] = s.d_il[5];
                    s.d_il[5] = s.d_il[4];
                    s.d_il[4] = input_sample_l;
                    input_sample_l = (s.d_ql[1] + s.d_il[5] + s.d_il[6]) / 3.0;
                    allpasstemp = s.out_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.o_jl[allpasstemp as usize] * constallpass;
                    s.o_jl[s.out_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_jl -= 1;
                    if s.out_jl < 0 || s.out_jl > s.delay_j {
                        s.out_jl = s.delay_j;
                    }
                    input_sample_l += s.o_jl[s.out_jl as usize];
                    s.d_jl[6] = s.d_jl[5];
                    s.d_jl[5] = s.d_jl[4];
                    s.d_jl[4] = input_sample_l;
                    input_sample_l = (s.d_pl[1] + s.d_jl[5] + s.d_jl[6]) / 3.0;
                    allpasstemp = s.out_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.o_kl[allpasstemp as usize] * constallpass;
                    s.o_kl[s.out_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_kl -= 1;
                    if s.out_kl < 0 || s.out_kl > s.delay_k {
                        s.out_kl = s.delay_k;
                    }
                    input_sample_l += s.o_kl[s.out_kl as usize];
                    s.d_kl[6] = s.d_kl[5];
                    s.d_kl[5] = s.d_kl[4];
                    s.d_kl[4] = input_sample_l;
                    input_sample_l = (s.d_ol[1] + s.d_kl[5] + s.d_kl[6]) / 3.0;
                    allpasstemp = s.out_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.o_ll[allpasstemp as usize] * constallpass;
                    s.o_ll[s.out_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ll -= 1;
                    if s.out_ll < 0 || s.out_ll > s.delay_l {
                        s.out_ll = s.delay_l;
                    }
                    input_sample_l += s.o_ll[s.out_ll as usize];
                    s.d_ll[6] = s.d_ll[5];
                    s.d_ll[5] = s.d_ll[4];
                    s.d_ll[4] = input_sample_l;
                    input_sample_l = (s.d_nl[1] + s.d_ll[5] + s.d_ll[6]) / 3.0;
                    allpasstemp = s.out_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.o_ml[allpasstemp as usize] * constallpass;
                    s.o_ml[s.out_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ml -= 1;
                    if s.out_ml < 0 || s.out_ml > s.delay_m {
                        s.out_ml = s.delay_m;
                    }
                    input_sample_l += s.o_ml[s.out_ml as usize];
                    s.d_ml[6] = s.d_ml[5];
                    s.d_ml[5] = s.d_ml[4];
                    s.d_ml[4] = input_sample_l;
                    input_sample_l = (s.d_ml[1] + s.d_ml[5] + s.d_ml[6]) / 3.0;
                    allpasstemp = s.out_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.o_nl[allpasstemp as usize] * constallpass;
                    s.o_nl[s.out_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_nl -= 1;
                    if s.out_nl < 0 || s.out_nl > s.delay_n {
                        s.out_nl = s.delay_n;
                    }
                    input_sample_l += s.o_nl[s.out_nl as usize];
                    s.d_nl[6] = s.d_nl[5];
                    s.d_nl[5] = s.d_nl[4];
                    s.d_nl[4] = input_sample_l;
                    input_sample_l = (s.d_ll[1] + s.d_nl[5] + s.d_nl[6]) / 3.0;
                    allpasstemp = s.out_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.o_ol[allpasstemp as usize] * constallpass;
                    s.o_ol[s.out_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ol -= 1;
                    if s.out_ol < 0 || s.out_ol > s.delay_o {
                        s.out_ol = s.delay_o;
                    }
                    input_sample_l += s.o_ol[s.out_ol as usize];
                    s.d_ol[6] = s.d_ol[5];
                    s.d_ol[5] = s.d_ol[4];
                    s.d_ol[4] = input_sample_l;
                    input_sample_l = (s.d_kl[1] + s.d_ol[5] + s.d_ol[6]) / 3.0;
                    allpasstemp = s.out_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.o_pl[allpasstemp as usize] * constallpass;
                    s.o_pl[s.out_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_pl -= 1;
                    if s.out_pl < 0 || s.out_pl > s.delay_p {
                        s.out_pl = s.delay_p;
                    }
                    input_sample_l += s.o_pl[s.out_pl as usize];
                    s.d_pl[6] = s.d_pl[5];
                    s.d_pl[5] = s.d_pl[4];
                    s.d_pl[4] = input_sample_l;
                    input_sample_l = (s.d_jl[1] + s.d_pl[5] + s.d_pl[6]) / 3.0;
                    allpasstemp = s.out_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.o_ql[allpasstemp as usize] * constallpass;
                    s.o_ql[s.out_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ql -= 1;
                    if s.out_ql < 0 || s.out_ql > s.delay_q {
                        s.out_ql = s.delay_q;
                    }
                    input_sample_l += s.o_ql[s.out_ql as usize];
                    s.d_ql[6] = s.d_ql[5];
                    s.d_ql[5] = s.d_ql[4];
                    s.d_ql[4] = input_sample_l;
                    input_sample_l = (s.d_il[1] + s.d_ql[5] + s.d_ql[6]) / 3.0;
                    allpasstemp = s.out_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.o_rl[allpasstemp as usize] * constallpass;
                    s.o_rl[s.out_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_rl -= 1;
                    if s.out_rl < 0 || s.out_rl > s.delay_r {
                        s.out_rl = s.delay_r;
                    }
                    input_sample_l += s.o_rl[s.out_rl as usize];
                    s.d_rl[6] = s.d_rl[5];
                    s.d_rl[5] = s.d_rl[4];
                    s.d_rl[4] = input_sample_l;
                    input_sample_l = (s.d_hl[1] + s.d_rl[5] + s.d_rl[6]) / 3.0;
                    allpasstemp = s.out_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.o_sl[allpasstemp as usize] * constallpass;
                    s.o_sl[s.out_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_sl -= 1;
                    if s.out_sl < 0 || s.out_sl > s.delay_s {
                        s.out_sl = s.delay_s;
                    }
                    input_sample_l += s.o_sl[s.out_sl as usize];
                    s.d_sl[6] = s.d_sl[5];
                    s.d_sl[5] = s.d_sl[4];
                    s.d_sl[4] = input_sample_l;
                    input_sample_l = (s.d_gl[1] + s.d_sl[5] + s.d_sl[6]) / 3.0;
                    allpasstemp = s.out_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.o_tl[allpasstemp as usize] * constallpass;
                    s.o_tl[s.out_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_tl -= 1;
                    if s.out_tl < 0 || s.out_tl > s.delay_t {
                        s.out_tl = s.delay_t;
                    }
                    input_sample_l += s.o_tl[s.out_tl as usize];
                    s.d_tl[6] = s.d_tl[5];
                    s.d_tl[5] = s.d_tl[4];
                    s.d_tl[4] = input_sample_l;
                    input_sample_l = (s.d_fl[1] + s.d_tl[5] + s.d_tl[6]) / 3.0;
                    allpasstemp = s.out_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.o_ul[allpasstemp as usize] * constallpass;
                    s.o_ul[s.out_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ul -= 1;
                    if s.out_ul < 0 || s.out_ul > s.delay_u {
                        s.out_ul = s.delay_u;
                    }
                    input_sample_l += s.o_ul[s.out_ul as usize];
                    s.d_ul[6] = s.d_ul[5];
                    s.d_ul[5] = s.d_ul[4];
                    s.d_ul[4] = input_sample_l;
                    input_sample_l = (s.d_el[1] + s.d_ul[5] + s.d_ul[6]) / 3.0;
                    allpasstemp = s.out_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.o_vl[allpasstemp as usize] * constallpass;
                    s.o_vl[s.out_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_vl -= 1;
                    if s.out_vl < 0 || s.out_vl > s.delay_v {
                        s.out_vl = s.delay_v;
                    }
                    input_sample_l += s.o_vl[s.out_vl as usize];
                    s.d_vl[6] = s.d_vl[5];
                    s.d_vl[5] = s.d_vl[4];
                    s.d_vl[4] = input_sample_l;
                    input_sample_l = (s.d_dl[1] + s.d_vl[5] + s.d_vl[6]) / 3.0;
                    allpasstemp = s.out_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.o_wl[allpasstemp as usize] * constallpass;
                    s.o_wl[s.out_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_wl -= 1;
                    if s.out_wl < 0 || s.out_wl > s.delay_w {
                        s.out_wl = s.delay_w;
                    }
                    input_sample_l += s.o_wl[s.out_wl as usize];
                    s.d_wl[6] = s.d_wl[5];
                    s.d_wl[5] = s.d_wl[4];
                    s.d_wl[4] = input_sample_l;
                    input_sample_l = (s.d_cl[1] + s.d_wl[5] + s.d_wl[6]) / 3.0;
                    allpasstemp = s.out_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.o_xl[allpasstemp as usize] * constallpass;
                    s.o_xl[s.out_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_xl -= 1;
                    if s.out_xl < 0 || s.out_xl > s.delay_x {
                        s.out_xl = s.delay_x;
                    }
                    input_sample_l += s.o_xl[s.out_xl as usize];
                    s.d_xl[6] = s.d_xl[5];
                    s.d_xl[5] = s.d_xl[4];
                    s.d_xl[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_xl[5] + s.d_xl[6]) / 3.0;
                    allpasstemp = s.out_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.o_yl[allpasstemp as usize] * constallpass;
                    s.o_yl[s.out_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_yl -= 1;
                    if s.out_yl < 0 || s.out_yl > s.delay_y {
                        s.out_yl = s.delay_y;
                    }
                    input_sample_l += s.o_yl[s.out_yl as usize];
                    s.d_yl[6] = s.d_yl[5];
                    s.d_yl[5] = s.d_yl[4];
                    s.d_yl[4] = input_sample_l;
                    input_sample_l = (s.d_bl[1] + s.d_yl[5] + s.d_yl[6]) / 3.0;
                    allpasstemp = s.out_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.o_zl[allpasstemp as usize] * constallpass;
                    s.o_zl[s.out_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_zl -= 1;
                    if s.out_zl < 0 || s.out_zl > s.delay_z {
                        s.out_zl = s.delay_z;
                    }
                    input_sample_l += s.o_zl[s.out_zl as usize];
                    s.d_zl[6] = s.d_zl[5];
                    s.d_zl[5] = s.d_zl[4];
                    s.d_zl[4] = input_sample_l;
                    input_sample_l = s.d_zl[5] + s.d_zl[6];
                }
                3 => {
                    allpasstemp = s.alp_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.a_al[allpasstemp as usize] * constallpass;
                    s.a_al[s.alp_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_al -= 1;
                    if s.alp_al < 0 || s.alp_al > s.delay_a {
                        s.alp_al = s.delay_a;
                    }
                    input_sample_l += s.a_al[s.alp_al as usize];
                    s.d_al[2] = s.d_al[1];
                    s.d_al[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_al[2]) / 2.0;
                    allpasstemp = s.alp_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.a_bl[allpasstemp as usize] * constallpass;
                    s.a_bl[s.alp_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_bl -= 1;
                    if s.alp_bl < 0 || s.alp_bl > s.delay_b {
                        s.alp_bl = s.delay_b;
                    }
                    input_sample_l += s.a_bl[s.alp_bl as usize];
                    s.d_bl[2] = s.d_bl[1];
                    s.d_bl[1] = input_sample_l;
                    input_sample_l = (s.d_bl[1] + s.d_bl[2]) / 2.0;
                    allpasstemp = s.alp_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.a_cl[allpasstemp as usize] * constallpass;
                    s.a_cl[s.alp_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_cl -= 1;
                    if s.alp_cl < 0 || s.alp_cl > s.delay_c {
                        s.alp_cl = s.delay_c;
                    }
                    input_sample_l += s.a_cl[s.alp_cl as usize];
                    s.d_cl[2] = s.d_cl[1];
                    s.d_cl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_cl[2]) / 2.0;
                    allpasstemp = s.alp_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.a_dl[allpasstemp as usize] * constallpass;
                    s.a_dl[s.alp_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_dl -= 1;
                    if s.alp_dl < 0 || s.alp_dl > s.delay_d {
                        s.alp_dl = s.delay_d;
                    }
                    input_sample_l += s.a_dl[s.alp_dl as usize];
                    s.d_dl[2] = s.d_dl[1];
                    s.d_dl[1] = input_sample_l;
                    input_sample_l = (s.d_dl[1] + s.d_dl[2]) / 2.0;
                    allpasstemp = s.alp_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.a_el[allpasstemp as usize] * constallpass;
                    s.a_el[s.alp_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_el -= 1;
                    if s.alp_el < 0 || s.alp_el > s.delay_e {
                        s.alp_el = s.delay_e;
                    }
                    input_sample_l += s.a_el[s.alp_el as usize];
                    s.d_el[2] = s.d_el[1];
                    s.d_el[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_el[2]) / 2.0;
                    allpasstemp = s.alp_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.a_fl[allpasstemp as usize] * constallpass;
                    s.a_fl[s.alp_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_fl -= 1;
                    if s.alp_fl < 0 || s.alp_fl > s.delay_f {
                        s.alp_fl = s.delay_f;
                    }
                    input_sample_l += s.a_fl[s.alp_fl as usize];
                    s.d_fl[2] = s.d_fl[1];
                    s.d_fl[1] = input_sample_l;
                    input_sample_l = (s.d_fl[1] + s.d_fl[2]) / 2.0;
                    allpasstemp = s.alp_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.a_gl[allpasstemp as usize] * constallpass;
                    s.a_gl[s.alp_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_gl -= 1;
                    if s.alp_gl < 0 || s.alp_gl > s.delay_g {
                        s.alp_gl = s.delay_g;
                    }
                    input_sample_l += s.a_gl[s.alp_gl as usize];
                    s.d_gl[2] = s.d_gl[1];
                    s.d_gl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_gl[2]) / 2.0;
                    allpasstemp = s.alp_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.a_hl[allpasstemp as usize] * constallpass;
                    s.a_hl[s.alp_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_hl -= 1;
                    if s.alp_hl < 0 || s.alp_hl > s.delay_h {
                        s.alp_hl = s.delay_h;
                    }
                    input_sample_l += s.a_hl[s.alp_hl as usize];
                    s.d_hl[2] = s.d_hl[1];
                    s.d_hl[1] = input_sample_l;
                    input_sample_l = (s.d_hl[1] + s.d_hl[2]) / 2.0;
                    allpasstemp = s.alp_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.a_il[allpasstemp as usize] * constallpass;
                    s.a_il[s.alp_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_il -= 1;
                    if s.alp_il < 0 || s.alp_il > s.delay_i {
                        s.alp_il = s.delay_i;
                    }
                    input_sample_l += s.a_il[s.alp_il as usize];
                    s.d_il[2] = s.d_il[1];
                    s.d_il[1] = input_sample_l;
                    input_sample_l = (s.d_il[1] + s.d_il[2]) / 2.0;
                    allpasstemp = s.alp_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.a_jl[allpasstemp as usize] * constallpass;
                    s.a_jl[s.alp_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_jl -= 1;
                    if s.alp_jl < 0 || s.alp_jl > s.delay_j {
                        s.alp_jl = s.delay_j;
                    }
                    input_sample_l += s.a_jl[s.alp_jl as usize];
                    s.d_jl[2] = s.d_jl[1];
                    s.d_jl[1] = input_sample_l;
                    input_sample_l = (s.d_jl[1] + s.d_jl[2]) / 2.0;
                    allpasstemp = s.alp_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.a_kl[allpasstemp as usize] * constallpass;
                    s.a_kl[s.alp_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_kl -= 1;
                    if s.alp_kl < 0 || s.alp_kl > s.delay_k {
                        s.alp_kl = s.delay_k;
                    }
                    input_sample_l += s.a_kl[s.alp_kl as usize];
                    s.d_kl[2] = s.d_kl[1];
                    s.d_kl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_kl[2]) / 2.0;
                    allpasstemp = s.alp_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.a_ll[allpasstemp as usize] * constallpass;
                    s.a_ll[s.alp_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ll -= 1;
                    if s.alp_ll < 0 || s.alp_ll > s.delay_l {
                        s.alp_ll = s.delay_l;
                    }
                    input_sample_l += s.a_ll[s.alp_ll as usize];
                    s.d_ll[2] = s.d_ll[1];
                    s.d_ll[1] = input_sample_l;
                    input_sample_l = (s.d_ll[1] + s.d_ll[2]) / 2.0;
                    allpasstemp = s.alp_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.a_ml[allpasstemp as usize] * constallpass;
                    s.a_ml[s.alp_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ml -= 1;
                    if s.alp_ml < 0 || s.alp_ml > s.delay_m {
                        s.alp_ml = s.delay_m;
                    }
                    input_sample_l += s.a_ml[s.alp_ml as usize];
                    s.d_ml[2] = s.d_ml[1];
                    s.d_ml[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ml[2]) / 2.0;
                    allpasstemp = s.alp_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.a_nl[allpasstemp as usize] * constallpass;
                    s.a_nl[s.alp_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_nl -= 1;
                    if s.alp_nl < 0 || s.alp_nl > s.delay_n {
                        s.alp_nl = s.delay_n;
                    }
                    input_sample_l += s.a_nl[s.alp_nl as usize];
                    s.d_nl[2] = s.d_nl[1];
                    s.d_nl[1] = input_sample_l;
                    input_sample_l = (s.d_nl[1] + s.d_nl[2]) / 2.0;
                    allpasstemp = s.alp_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.a_ol[allpasstemp as usize] * constallpass;
                    s.a_ol[s.alp_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ol -= 1;
                    if s.alp_ol < 0 || s.alp_ol > s.delay_o {
                        s.alp_ol = s.delay_o;
                    }
                    input_sample_l += s.a_ol[s.alp_ol as usize];
                    s.d_ol[2] = s.d_ol[1];
                    s.d_ol[1] = input_sample_l;
                    input_sample_l = (s.d_ol[1] + s.d_ol[2]) / 2.0;
                    allpasstemp = s.alp_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.a_pl[allpasstemp as usize] * constallpass;
                    s.a_pl[s.alp_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_pl -= 1;
                    if s.alp_pl < 0 || s.alp_pl > s.delay_p {
                        s.alp_pl = s.delay_p;
                    }
                    input_sample_l += s.a_pl[s.alp_pl as usize];
                    s.d_pl[2] = s.d_pl[1];
                    s.d_pl[1] = input_sample_l;
                    input_sample_l = (s.d_pl[1] + s.d_pl[2]) / 2.0;
                    allpasstemp = s.alp_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.a_ql[allpasstemp as usize] * constallpass;
                    s.a_ql[s.alp_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ql -= 1;
                    if s.alp_ql < 0 || s.alp_ql > s.delay_q {
                        s.alp_ql = s.delay_q;
                    }
                    input_sample_l += s.a_ql[s.alp_ql as usize];
                    s.d_ql[2] = s.d_ql[1];
                    s.d_ql[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ql[2]) / 2.0;
                    allpasstemp = s.alp_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.a_rl[allpasstemp as usize] * constallpass;
                    s.a_rl[s.alp_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_rl -= 1;
                    if s.alp_rl < 0 || s.alp_rl > s.delay_r {
                        s.alp_rl = s.delay_r;
                    }
                    input_sample_l += s.a_rl[s.alp_rl as usize];
                    s.d_rl[2] = s.d_rl[1];
                    s.d_rl[1] = input_sample_l;
                    input_sample_l = (s.d_rl[1] + s.d_rl[2]) / 2.0;
                    allpasstemp = s.alp_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.a_sl[allpasstemp as usize] * constallpass;
                    s.a_sl[s.alp_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_sl -= 1;
                    if s.alp_sl < 0 || s.alp_sl > s.delay_s {
                        s.alp_sl = s.delay_s;
                    }
                    input_sample_l += s.a_sl[s.alp_sl as usize];
                    s.d_sl[2] = s.d_sl[1];
                    s.d_sl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_sl[2]) / 2.0;
                    allpasstemp = s.alp_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.a_tl[allpasstemp as usize] * constallpass;
                    s.a_tl[s.alp_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_tl -= 1;
                    if s.alp_tl < 0 || s.alp_tl > s.delay_t {
                        s.alp_tl = s.delay_t;
                    }
                    input_sample_l += s.a_tl[s.alp_tl as usize];
                    s.d_tl[2] = s.d_tl[1];
                    s.d_tl[1] = input_sample_l;
                    input_sample_l = (s.d_tl[1] + s.d_tl[2]) / 2.0;
                    allpasstemp = s.alp_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.a_ul[allpasstemp as usize] * constallpass;
                    s.a_ul[s.alp_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ul -= 1;
                    if s.alp_ul < 0 || s.alp_ul > s.delay_u {
                        s.alp_ul = s.delay_u;
                    }
                    input_sample_l += s.a_ul[s.alp_ul as usize];
                    s.d_ul[2] = s.d_ul[1];
                    s.d_ul[1] = input_sample_l;
                    input_sample_l = (s.d_ul[1] + s.d_ul[2]) / 2.0;
                    allpasstemp = s.alp_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.a_vl[allpasstemp as usize] * constallpass;
                    s.a_vl[s.alp_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_vl -= 1;
                    if s.alp_vl < 0 || s.alp_vl > s.delay_v {
                        s.alp_vl = s.delay_v;
                    }
                    input_sample_l += s.a_vl[s.alp_vl as usize];
                    s.d_vl[2] = s.d_vl[1];
                    s.d_vl[1] = input_sample_l;
                    input_sample_l = (s.d_vl[1] + s.d_vl[2]) / 2.0;
                    allpasstemp = s.alp_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.a_wl[allpasstemp as usize] * constallpass;
                    s.a_wl[s.alp_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_wl -= 1;
                    if s.alp_wl < 0 || s.alp_wl > s.delay_w {
                        s.alp_wl = s.delay_w;
                    }
                    input_sample_l += s.a_wl[s.alp_wl as usize];
                    s.d_wl[2] = s.d_wl[1];
                    s.d_wl[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_wl[2]) / 2.0;
                    allpasstemp = s.alp_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.a_xl[allpasstemp as usize] * constallpass;
                    s.a_xl[s.alp_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_xl -= 1;
                    if s.alp_xl < 0 || s.alp_xl > s.delay_x {
                        s.alp_xl = s.delay_x;
                    }
                    input_sample_l += s.a_xl[s.alp_xl as usize];
                    s.d_xl[2] = s.d_xl[1];
                    s.d_xl[1] = input_sample_l;
                    input_sample_l = (s.d_xl[1] + s.d_xl[2]) / 2.0;
                    allpasstemp = s.alp_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.a_yl[allpasstemp as usize] * constallpass;
                    s.a_yl[s.alp_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_yl -= 1;
                    if s.alp_yl < 0 || s.alp_yl > s.delay_y {
                        s.alp_yl = s.delay_y;
                    }
                    input_sample_l += s.a_yl[s.alp_yl as usize];
                    s.d_yl[2] = s.d_yl[1];
                    s.d_yl[1] = input_sample_l;
                    input_sample_l = (s.d_yl[1] + s.d_yl[2]) / 2.0;
                    allpasstemp = s.alp_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.a_zl[allpasstemp as usize] * constallpass;
                    s.a_zl[s.alp_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_zl -= 1;
                    if s.alp_zl < 0 || s.alp_zl > s.delay_z {
                        s.alp_zl = s.delay_z;
                    }
                    input_sample_l += s.a_zl[s.alp_zl as usize];
                    s.d_zl[2] = s.d_zl[1];
                    s.d_zl[1] = input_sample_l;
                    input_sample_l = (s.d_zl[1] + s.d_zl[2]) / 2.0;
                    allpasstemp = s.out_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.o_al[allpasstemp as usize] * constallpass;
                    s.o_al[s.out_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_al -= 1;
                    if s.out_al < 0 || s.out_al > s.delay_a {
                        s.out_al = s.delay_a;
                    }
                    input_sample_l += s.o_al[s.out_al as usize];
                    s.d_al[5] = s.d_al[4];
                    s.d_al[4] = input_sample_l;
                    input_sample_l = (s.d_al[4] + s.d_al[5]) / 2.0;
                    allpasstemp = s.out_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.o_bl[allpasstemp as usize] * constallpass;
                    s.o_bl[s.out_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_bl -= 1;
                    if s.out_bl < 0 || s.out_bl > s.delay_b {
                        s.out_bl = s.delay_b;
                    }
                    input_sample_l += s.o_bl[s.out_bl as usize];
                    s.d_bl[5] = s.d_bl[4];
                    s.d_bl[4] = input_sample_l;
                    input_sample_l = (s.d_bl[4] + s.d_bl[5]) / 2.0;
                    allpasstemp = s.out_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.o_cl[allpasstemp as usize] * constallpass;
                    s.o_cl[s.out_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_cl -= 1;
                    if s.out_cl < 0 || s.out_cl > s.delay_c {
                        s.out_cl = s.delay_c;
                    }
                    input_sample_l += s.o_cl[s.out_cl as usize];
                    s.d_cl[5] = s.d_cl[4];
                    s.d_cl[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_cl[5]) / 2.0;
                    allpasstemp = s.out_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.o_dl[allpasstemp as usize] * constallpass;
                    s.o_dl[s.out_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_dl -= 1;
                    if s.out_dl < 0 || s.out_dl > s.delay_d {
                        s.out_dl = s.delay_d;
                    }
                    input_sample_l += s.o_dl[s.out_dl as usize];
                    s.d_dl[5] = s.d_dl[4];
                    s.d_dl[4] = input_sample_l;
                    input_sample_l = (s.d_dl[4] + s.d_dl[5]) / 2.0;
                    allpasstemp = s.out_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.o_el[allpasstemp as usize] * constallpass;
                    s.o_el[s.out_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_el -= 1;
                    if s.out_el < 0 || s.out_el > s.delay_e {
                        s.out_el = s.delay_e;
                    }
                    input_sample_l += s.o_el[s.out_el as usize];
                    s.d_el[5] = s.d_el[4];
                    s.d_el[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_el[5]) / 2.0;
                    allpasstemp = s.out_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.o_fl[allpasstemp as usize] * constallpass;
                    s.o_fl[s.out_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_fl -= 1;
                    if s.out_fl < 0 || s.out_fl > s.delay_f {
                        s.out_fl = s.delay_f;
                    }
                    input_sample_l += s.o_fl[s.out_fl as usize];
                    s.d_fl[5] = s.d_fl[4];
                    s.d_fl[4] = input_sample_l;
                    input_sample_l = (s.d_fl[4] + s.d_fl[5]) / 2.0;
                    allpasstemp = s.out_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.o_gl[allpasstemp as usize] * constallpass;
                    s.o_gl[s.out_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_gl -= 1;
                    if s.out_gl < 0 || s.out_gl > s.delay_g {
                        s.out_gl = s.delay_g;
                    }
                    input_sample_l += s.o_gl[s.out_gl as usize];
                    s.d_gl[5] = s.d_gl[4];
                    s.d_gl[4] = input_sample_l;
                    input_sample_l = (s.d_gl[4] + s.d_gl[5]) / 2.0;
                    allpasstemp = s.out_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.o_hl[allpasstemp as usize] * constallpass;
                    s.o_hl[s.out_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_hl -= 1;
                    if s.out_hl < 0 || s.out_hl > s.delay_h {
                        s.out_hl = s.delay_h;
                    }
                    input_sample_l += s.o_hl[s.out_hl as usize];
                    s.d_hl[5] = s.d_hl[4];
                    s.d_hl[4] = input_sample_l;
                    input_sample_l = (s.d_hl[4] + s.d_hl[5]) / 2.0;
                    allpasstemp = s.out_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.o_il[allpasstemp as usize] * constallpass;
                    s.o_il[s.out_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_il -= 1;
                    if s.out_il < 0 || s.out_il > s.delay_i {
                        s.out_il = s.delay_i;
                    }
                    input_sample_l += s.o_il[s.out_il as usize];
                    s.d_il[5] = s.d_il[4];
                    s.d_il[4] = input_sample_l;
                    input_sample_l = (s.d_il[4] + s.d_il[5]) / 2.0;
                    allpasstemp = s.out_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.o_jl[allpasstemp as usize] * constallpass;
                    s.o_jl[s.out_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_jl -= 1;
                    if s.out_jl < 0 || s.out_jl > s.delay_j {
                        s.out_jl = s.delay_j;
                    }
                    input_sample_l += s.o_jl[s.out_jl as usize];
                    s.d_jl[5] = s.d_jl[4];
                    s.d_jl[4] = input_sample_l;
                    input_sample_l = (s.d_jl[4] + s.d_jl[5]) / 2.0;
                    allpasstemp = s.out_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.o_kl[allpasstemp as usize] * constallpass;
                    s.o_kl[s.out_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_kl -= 1;
                    if s.out_kl < 0 || s.out_kl > s.delay_k {
                        s.out_kl = s.delay_k;
                    }
                    input_sample_l += s.o_kl[s.out_kl as usize];
                    s.d_kl[5] = s.d_kl[4];
                    s.d_kl[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_kl[5]) / 2.0;
                    allpasstemp = s.out_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.o_ll[allpasstemp as usize] * constallpass;
                    s.o_ll[s.out_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ll -= 1;
                    if s.out_ll < 0 || s.out_ll > s.delay_l {
                        s.out_ll = s.delay_l;
                    }
                    input_sample_l += s.o_ll[s.out_ll as usize];
                    s.d_ll[5] = s.d_ll[4];
                    s.d_ll[4] = input_sample_l;
                    input_sample_l = (s.d_ll[4] + s.d_ll[5]) / 2.0;
                    allpasstemp = s.out_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.o_ml[allpasstemp as usize] * constallpass;
                    s.o_ml[s.out_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ml -= 1;
                    if s.out_ml < 0 || s.out_ml > s.delay_m {
                        s.out_ml = s.delay_m;
                    }
                    input_sample_l += s.o_ml[s.out_ml as usize];
                    s.d_ml[5] = s.d_ml[4];
                    s.d_ml[4] = input_sample_l;
                    input_sample_l = (s.d_ml[4] + s.d_ml[5]) / 2.0;
                    allpasstemp = s.out_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.o_nl[allpasstemp as usize] * constallpass;
                    s.o_nl[s.out_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_nl -= 1;
                    if s.out_nl < 0 || s.out_nl > s.delay_n {
                        s.out_nl = s.delay_n;
                    }
                    input_sample_l += s.o_nl[s.out_nl as usize];
                    s.d_nl[5] = s.d_nl[4];
                    s.d_nl[4] = input_sample_l;
                    input_sample_l = (s.d_nl[4] + s.d_nl[5]) / 2.0;
                    allpasstemp = s.out_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.o_ol[allpasstemp as usize] * constallpass;
                    s.o_ol[s.out_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ol -= 1;
                    if s.out_ol < 0 || s.out_ol > s.delay_o {
                        s.out_ol = s.delay_o;
                    }
                    input_sample_l += s.o_ol[s.out_ol as usize];
                    s.d_ol[5] = s.d_ol[4];
                    s.d_ol[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ol[5]) / 2.0;
                    allpasstemp = s.out_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.o_pl[allpasstemp as usize] * constallpass;
                    s.o_pl[s.out_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_pl -= 1;
                    if s.out_pl < 0 || s.out_pl > s.delay_p {
                        s.out_pl = s.delay_p;
                    }
                    input_sample_l += s.o_pl[s.out_pl as usize];
                    s.d_pl[5] = s.d_pl[4];
                    s.d_pl[4] = input_sample_l;
                    input_sample_l = (s.d_pl[4] + s.d_pl[5]) / 2.0;
                    allpasstemp = s.out_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.o_ql[allpasstemp as usize] * constallpass;
                    s.o_ql[s.out_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ql -= 1;
                    if s.out_ql < 0 || s.out_ql > s.delay_q {
                        s.out_ql = s.delay_q;
                    }
                    input_sample_l += s.o_ql[s.out_ql as usize];
                    s.d_ql[5] = s.d_ql[4];
                    s.d_ql[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ql[5]) / 2.0;
                    allpasstemp = s.out_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.o_rl[allpasstemp as usize] * constallpass;
                    s.o_rl[s.out_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_rl -= 1;
                    if s.out_rl < 0 || s.out_rl > s.delay_r {
                        s.out_rl = s.delay_r;
                    }
                    input_sample_l += s.o_rl[s.out_rl as usize];
                    s.d_rl[5] = s.d_rl[4];
                    s.d_rl[4] = input_sample_l;
                    input_sample_l = (s.d_rl[4] + s.d_rl[5]) / 2.0;
                    allpasstemp = s.out_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.o_sl[allpasstemp as usize] * constallpass;
                    s.o_sl[s.out_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_sl -= 1;
                    if s.out_sl < 0 || s.out_sl > s.delay_s {
                        s.out_sl = s.delay_s;
                    }
                    input_sample_l += s.o_sl[s.out_sl as usize];
                    s.d_sl[5] = s.d_sl[4];
                    s.d_sl[4] = input_sample_l;
                    input_sample_l = (s.d_sl[4] + s.d_sl[5]) / 2.0;
                    allpasstemp = s.out_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.o_tl[allpasstemp as usize] * constallpass;
                    s.o_tl[s.out_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_tl -= 1;
                    if s.out_tl < 0 || s.out_tl > s.delay_t {
                        s.out_tl = s.delay_t;
                    }
                    input_sample_l += s.o_tl[s.out_tl as usize];
                    s.d_tl[5] = s.d_tl[4];
                    s.d_tl[4] = input_sample_l;
                    input_sample_l = (s.d_tl[4] + s.d_tl[5]) / 2.0;
                    allpasstemp = s.out_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.o_ul[allpasstemp as usize] * constallpass;
                    s.o_ul[s.out_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ul -= 1;
                    if s.out_ul < 0 || s.out_ul > s.delay_u {
                        s.out_ul = s.delay_u;
                    }
                    input_sample_l += s.o_ul[s.out_ul as usize];
                    s.d_ul[5] = s.d_ul[4];
                    s.d_ul[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_ul[5]) / 2.0;
                    allpasstemp = s.out_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.o_vl[allpasstemp as usize] * constallpass;
                    s.o_vl[s.out_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_vl -= 1;
                    if s.out_vl < 0 || s.out_vl > s.delay_v {
                        s.out_vl = s.delay_v;
                    }
                    input_sample_l += s.o_vl[s.out_vl as usize];
                    s.d_vl[5] = s.d_vl[4];
                    s.d_vl[4] = input_sample_l;
                    input_sample_l = (s.d_vl[4] + s.d_vl[5]) / 2.0;
                    allpasstemp = s.out_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.o_wl[allpasstemp as usize] * constallpass;
                    s.o_wl[s.out_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_wl -= 1;
                    if s.out_wl < 0 || s.out_wl > s.delay_w {
                        s.out_wl = s.delay_w;
                    }
                    input_sample_l += s.o_wl[s.out_wl as usize];
                    s.d_wl[5] = s.d_wl[4];
                    s.d_wl[4] = input_sample_l;
                    input_sample_l = (s.d_wl[4] + s.d_wl[5]) / 2.0;
                    allpasstemp = s.out_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.o_xl[allpasstemp as usize] * constallpass;
                    s.o_xl[s.out_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_xl -= 1;
                    if s.out_xl < 0 || s.out_xl > s.delay_x {
                        s.out_xl = s.delay_x;
                    }
                    input_sample_l += s.o_xl[s.out_xl as usize];
                    s.d_xl[5] = s.d_xl[4];
                    s.d_xl[4] = input_sample_l;
                    input_sample_l = (s.d_xl[4] + s.d_xl[5]) / 2.0;
                    allpasstemp = s.out_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.o_yl[allpasstemp as usize] * constallpass;
                    s.o_yl[s.out_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_yl -= 1;
                    if s.out_yl < 0 || s.out_yl > s.delay_y {
                        s.out_yl = s.delay_y;
                    }
                    input_sample_l += s.o_yl[s.out_yl as usize];
                    s.d_yl[5] = s.d_yl[4];
                    s.d_yl[4] = input_sample_l;
                    input_sample_l = (s.d_yl[4] + s.d_yl[5]) / 2.0;
                    allpasstemp = s.out_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.o_zl[allpasstemp as usize] * constallpass;
                    s.o_zl[s.out_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_zl -= 1;
                    if s.out_zl < 0 || s.out_zl > s.delay_z {
                        s.out_zl = s.delay_z;
                    }
                    input_sample_l += s.o_zl[s.out_zl as usize];
                    s.d_zl[5] = s.d_zl[4];
                    s.d_zl[4] = input_sample_l;
                    input_sample_l = s.d_zl[4] + s.d_zl[5];
                }
                4 => {
                    allpasstemp = s.alp_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.a_al[allpasstemp as usize] * constallpass;
                    s.a_al[s.alp_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_al -= 1;
                    if s.alp_al < 0 || s.alp_al > s.delay_a {
                        s.alp_al = s.delay_a;
                    }
                    input_sample_l += s.a_al[s.alp_al as usize];
                    s.d_al[2] = s.d_al[1];
                    s.d_al[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.a_bl[allpasstemp as usize] * constallpass;
                    s.a_bl[s.alp_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_bl -= 1;
                    if s.alp_bl < 0 || s.alp_bl > s.delay_b {
                        s.alp_bl = s.delay_b;
                    }
                    input_sample_l += s.a_bl[s.alp_bl as usize];
                    s.d_bl[2] = s.d_bl[1];
                    s.d_bl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.a_cl[allpasstemp as usize] * constallpass;
                    s.a_cl[s.alp_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_cl -= 1;
                    if s.alp_cl < 0 || s.alp_cl > s.delay_c {
                        s.alp_cl = s.delay_c;
                    }
                    input_sample_l += s.a_cl[s.alp_cl as usize];
                    s.d_cl[2] = s.d_cl[1];
                    s.d_cl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.a_dl[allpasstemp as usize] * constallpass;
                    s.a_dl[s.alp_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_dl -= 1;
                    if s.alp_dl < 0 || s.alp_dl > s.delay_d {
                        s.alp_dl = s.delay_d;
                    }
                    input_sample_l += s.a_dl[s.alp_dl as usize];
                    s.d_dl[2] = s.d_dl[1];
                    s.d_dl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.a_el[allpasstemp as usize] * constallpass;
                    s.a_el[s.alp_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_el -= 1;
                    if s.alp_el < 0 || s.alp_el > s.delay_e {
                        s.alp_el = s.delay_e;
                    }
                    input_sample_l += s.a_el[s.alp_el as usize];
                    s.d_el[2] = s.d_el[1];
                    s.d_el[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.a_fl[allpasstemp as usize] * constallpass;
                    s.a_fl[s.alp_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_fl -= 1;
                    if s.alp_fl < 0 || s.alp_fl > s.delay_f {
                        s.alp_fl = s.delay_f;
                    }
                    input_sample_l += s.a_fl[s.alp_fl as usize];
                    s.d_fl[2] = s.d_fl[1];
                    s.d_fl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.a_gl[allpasstemp as usize] * constallpass;
                    s.a_gl[s.alp_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_gl -= 1;
                    if s.alp_gl < 0 || s.alp_gl > s.delay_g {
                        s.alp_gl = s.delay_g;
                    }
                    input_sample_l += s.a_gl[s.alp_gl as usize];
                    s.d_gl[2] = s.d_gl[1];
                    s.d_gl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.a_hl[allpasstemp as usize] * constallpass;
                    s.a_hl[s.alp_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_hl -= 1;
                    if s.alp_hl < 0 || s.alp_hl > s.delay_h {
                        s.alp_hl = s.delay_h;
                    }
                    input_sample_l += s.a_hl[s.alp_hl as usize];
                    s.d_hl[2] = s.d_hl[1];
                    s.d_hl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.a_il[allpasstemp as usize] * constallpass;
                    s.a_il[s.alp_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_il -= 1;
                    if s.alp_il < 0 || s.alp_il > s.delay_i {
                        s.alp_il = s.delay_i;
                    }
                    input_sample_l += s.a_il[s.alp_il as usize];
                    s.d_il[2] = s.d_il[1];
                    s.d_il[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.a_jl[allpasstemp as usize] * constallpass;
                    s.a_jl[s.alp_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_jl -= 1;
                    if s.alp_jl < 0 || s.alp_jl > s.delay_j {
                        s.alp_jl = s.delay_j;
                    }
                    input_sample_l += s.a_jl[s.alp_jl as usize];
                    s.d_jl[2] = s.d_jl[1];
                    s.d_jl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.a_kl[allpasstemp as usize] * constallpass;
                    s.a_kl[s.alp_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_kl -= 1;
                    if s.alp_kl < 0 || s.alp_kl > s.delay_k {
                        s.alp_kl = s.delay_k;
                    }
                    input_sample_l += s.a_kl[s.alp_kl as usize];
                    s.d_kl[2] = s.d_kl[1];
                    s.d_kl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.a_ll[allpasstemp as usize] * constallpass;
                    s.a_ll[s.alp_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ll -= 1;
                    if s.alp_ll < 0 || s.alp_ll > s.delay_l {
                        s.alp_ll = s.delay_l;
                    }
                    input_sample_l += s.a_ll[s.alp_ll as usize];
                    s.d_ll[2] = s.d_ll[1];
                    s.d_ll[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.a_ml[allpasstemp as usize] * constallpass;
                    s.a_ml[s.alp_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ml -= 1;
                    if s.alp_ml < 0 || s.alp_ml > s.delay_m {
                        s.alp_ml = s.delay_m;
                    }
                    input_sample_l += s.a_ml[s.alp_ml as usize];
                    s.d_ml[2] = s.d_ml[1];
                    s.d_ml[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.a_nl[allpasstemp as usize] * constallpass;
                    s.a_nl[s.alp_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_nl -= 1;
                    if s.alp_nl < 0 || s.alp_nl > s.delay_n {
                        s.alp_nl = s.delay_n;
                    }
                    input_sample_l += s.a_nl[s.alp_nl as usize];
                    s.d_nl[2] = s.d_nl[1];
                    s.d_nl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.a_ol[allpasstemp as usize] * constallpass;
                    s.a_ol[s.alp_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ol -= 1;
                    if s.alp_ol < 0 || s.alp_ol > s.delay_o {
                        s.alp_ol = s.delay_o;
                    }
                    input_sample_l += s.a_ol[s.alp_ol as usize];
                    s.d_ol[2] = s.d_ol[1];
                    s.d_ol[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.a_pl[allpasstemp as usize] * constallpass;
                    s.a_pl[s.alp_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_pl -= 1;
                    if s.alp_pl < 0 || s.alp_pl > s.delay_p {
                        s.alp_pl = s.delay_p;
                    }
                    input_sample_l += s.a_pl[s.alp_pl as usize];
                    s.d_pl[2] = s.d_pl[1];
                    s.d_pl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.a_ql[allpasstemp as usize] * constallpass;
                    s.a_ql[s.alp_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ql -= 1;
                    if s.alp_ql < 0 || s.alp_ql > s.delay_q {
                        s.alp_ql = s.delay_q;
                    }
                    input_sample_l += s.a_ql[s.alp_ql as usize];
                    s.d_ql[2] = s.d_ql[1];
                    s.d_ql[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.a_rl[allpasstemp as usize] * constallpass;
                    s.a_rl[s.alp_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_rl -= 1;
                    if s.alp_rl < 0 || s.alp_rl > s.delay_r {
                        s.alp_rl = s.delay_r;
                    }
                    input_sample_l += s.a_rl[s.alp_rl as usize];
                    s.d_rl[2] = s.d_rl[1];
                    s.d_rl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.a_sl[allpasstemp as usize] * constallpass;
                    s.a_sl[s.alp_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_sl -= 1;
                    if s.alp_sl < 0 || s.alp_sl > s.delay_s {
                        s.alp_sl = s.delay_s;
                    }
                    input_sample_l += s.a_sl[s.alp_sl as usize];
                    s.d_sl[2] = s.d_sl[1];
                    s.d_sl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.a_tl[allpasstemp as usize] * constallpass;
                    s.a_tl[s.alp_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_tl -= 1;
                    if s.alp_tl < 0 || s.alp_tl > s.delay_t {
                        s.alp_tl = s.delay_t;
                    }
                    input_sample_l += s.a_tl[s.alp_tl as usize];
                    s.d_tl[2] = s.d_tl[1];
                    s.d_tl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.a_ul[allpasstemp as usize] * constallpass;
                    s.a_ul[s.alp_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ul -= 1;
                    if s.alp_ul < 0 || s.alp_ul > s.delay_u {
                        s.alp_ul = s.delay_u;
                    }
                    input_sample_l += s.a_ul[s.alp_ul as usize];
                    s.d_ul[2] = s.d_ul[1];
                    s.d_ul[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.a_vl[allpasstemp as usize] * constallpass;
                    s.a_vl[s.alp_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_vl -= 1;
                    if s.alp_vl < 0 || s.alp_vl > s.delay_v {
                        s.alp_vl = s.delay_v;
                    }
                    input_sample_l += s.a_vl[s.alp_vl as usize];
                    s.d_vl[2] = s.d_vl[1];
                    s.d_vl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.a_wl[allpasstemp as usize] * constallpass;
                    s.a_wl[s.alp_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_wl -= 1;
                    if s.alp_wl < 0 || s.alp_wl > s.delay_w {
                        s.alp_wl = s.delay_w;
                    }
                    input_sample_l += s.a_wl[s.alp_wl as usize];
                    s.d_wl[2] = s.d_wl[1];
                    s.d_wl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.a_xl[allpasstemp as usize] * constallpass;
                    s.a_xl[s.alp_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_xl -= 1;
                    if s.alp_xl < 0 || s.alp_xl > s.delay_x {
                        s.alp_xl = s.delay_x;
                    }
                    input_sample_l += s.a_xl[s.alp_xl as usize];
                    s.d_xl[2] = s.d_xl[1];
                    s.d_xl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.a_yl[allpasstemp as usize] * constallpass;
                    s.a_yl[s.alp_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_yl -= 1;
                    if s.alp_yl < 0 || s.alp_yl > s.delay_y {
                        s.alp_yl = s.delay_y;
                    }
                    input_sample_l += s.a_yl[s.alp_yl as usize];
                    s.d_yl[2] = s.d_yl[1];
                    s.d_yl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.alp_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.a_zl[allpasstemp as usize] * constallpass;
                    s.a_zl[s.alp_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_zl -= 1;
                    if s.alp_zl < 0 || s.alp_zl > s.delay_z {
                        s.alp_zl = s.delay_z;
                    }
                    input_sample_l += s.a_zl[s.alp_zl as usize];
                    s.d_zl[2] = s.d_zl[1];
                    s.d_zl[1] = input_sample_l;
                    input_sample_l = dry_sample_l;
                    allpasstemp = s.out_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.o_al[allpasstemp as usize] * constallpass;
                    s.o_al[s.out_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_al -= 1;
                    if s.out_al < 0 || s.out_al > s.delay_a {
                        s.out_al = s.delay_a;
                    }
                    input_sample_l += s.o_al[s.out_al as usize];
                    s.d_al[5] = s.d_al[4];
                    s.d_al[4] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_al[2]) / 2.0;
                    allpasstemp = s.out_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.o_bl[allpasstemp as usize] * constallpass;
                    s.o_bl[s.out_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_bl -= 1;
                    if s.out_bl < 0 || s.out_bl > s.delay_b {
                        s.out_bl = s.delay_b;
                    }
                    input_sample_l += s.o_bl[s.out_bl as usize];
                    s.d_bl[5] = s.d_bl[4];
                    s.d_bl[4] = input_sample_l;
                    input_sample_l = (s.d_bl[1] + s.d_bl[2]) / 2.0;
                    allpasstemp = s.out_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.o_cl[allpasstemp as usize] * constallpass;
                    s.o_cl[s.out_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_cl -= 1;
                    if s.out_cl < 0 || s.out_cl > s.delay_c {
                        s.out_cl = s.delay_c;
                    }
                    input_sample_l += s.o_cl[s.out_cl as usize];
                    s.d_cl[5] = s.d_cl[4];
                    s.d_cl[4] = input_sample_l;
                    input_sample_l = (s.d_cl[1] + s.d_cl[2]) / 2.0;
                    allpasstemp = s.out_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.o_dl[allpasstemp as usize] * constallpass;
                    s.o_dl[s.out_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_dl -= 1;
                    if s.out_dl < 0 || s.out_dl > s.delay_d {
                        s.out_dl = s.delay_d;
                    }
                    input_sample_l += s.o_dl[s.out_dl as usize];
                    s.d_dl[5] = s.d_dl[4];
                    s.d_dl[4] = input_sample_l;
                    input_sample_l = (s.d_dl[1] + s.d_dl[2]) / 2.0;
                    allpasstemp = s.out_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.o_el[allpasstemp as usize] * constallpass;
                    s.o_el[s.out_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_el -= 1;
                    if s.out_el < 0 || s.out_el > s.delay_e {
                        s.out_el = s.delay_e;
                    }
                    input_sample_l += s.o_el[s.out_el as usize];
                    s.d_el[5] = s.d_el[4];
                    s.d_el[4] = input_sample_l;
                    input_sample_l = (s.d_el[1] + s.d_el[2]) / 2.0;
                    allpasstemp = s.out_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.o_fl[allpasstemp as usize] * constallpass;
                    s.o_fl[s.out_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_fl -= 1;
                    if s.out_fl < 0 || s.out_fl > s.delay_f {
                        s.out_fl = s.delay_f;
                    }
                    input_sample_l += s.o_fl[s.out_fl as usize];
                    s.d_fl[5] = s.d_fl[4];
                    s.d_fl[4] = input_sample_l;
                    input_sample_l = (s.d_fl[1] + s.d_fl[2]) / 2.0;
                    allpasstemp = s.out_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.o_gl[allpasstemp as usize] * constallpass;
                    s.o_gl[s.out_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_gl -= 1;
                    if s.out_gl < 0 || s.out_gl > s.delay_g {
                        s.out_gl = s.delay_g;
                    }
                    input_sample_l += s.o_gl[s.out_gl as usize];
                    s.d_gl[5] = s.d_gl[4];
                    s.d_gl[4] = input_sample_l;
                    input_sample_l = (s.d_gl[1] + s.d_gl[2]) / 2.0;
                    allpasstemp = s.out_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.o_hl[allpasstemp as usize] * constallpass;
                    s.o_hl[s.out_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_hl -= 1;
                    if s.out_hl < 0 || s.out_hl > s.delay_h {
                        s.out_hl = s.delay_h;
                    }
                    input_sample_l += s.o_hl[s.out_hl as usize];
                    s.d_hl[5] = s.d_hl[4];
                    s.d_hl[4] = input_sample_l;
                    input_sample_l = (s.d_hl[1] + s.d_hl[2]) / 2.0;
                    allpasstemp = s.out_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.o_il[allpasstemp as usize] * constallpass;
                    s.o_il[s.out_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_il -= 1;
                    if s.out_il < 0 || s.out_il > s.delay_i {
                        s.out_il = s.delay_i;
                    }
                    input_sample_l += s.o_il[s.out_il as usize];
                    s.d_il[5] = s.d_il[4];
                    s.d_il[4] = input_sample_l;
                    input_sample_l = (s.d_il[1] + s.d_il[2]) / 2.0;
                    allpasstemp = s.out_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.o_jl[allpasstemp as usize] * constallpass;
                    s.o_jl[s.out_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_jl -= 1;
                    if s.out_jl < 0 || s.out_jl > s.delay_j {
                        s.out_jl = s.delay_j;
                    }
                    input_sample_l += s.o_jl[s.out_jl as usize];
                    s.d_jl[5] = s.d_jl[4];
                    s.d_jl[4] = input_sample_l;
                    input_sample_l = (s.d_jl[1] + s.d_jl[2]) / 2.0;
                    allpasstemp = s.out_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.o_kl[allpasstemp as usize] * constallpass;
                    s.o_kl[s.out_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_kl -= 1;
                    if s.out_kl < 0 || s.out_kl > s.delay_k {
                        s.out_kl = s.delay_k;
                    }
                    input_sample_l += s.o_kl[s.out_kl as usize];
                    s.d_kl[5] = s.d_kl[4];
                    s.d_kl[4] = input_sample_l;
                    input_sample_l = (s.d_kl[1] + s.d_kl[2]) / 2.0;
                    allpasstemp = s.out_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.o_ll[allpasstemp as usize] * constallpass;
                    s.o_ll[s.out_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ll -= 1;
                    if s.out_ll < 0 || s.out_ll > s.delay_l {
                        s.out_ll = s.delay_l;
                    }
                    input_sample_l += s.o_ll[s.out_ll as usize];
                    s.d_ll[5] = s.d_ll[4];
                    s.d_ll[4] = input_sample_l;
                    input_sample_l = (s.d_ll[1] + s.d_ll[2]) / 2.0;
                    allpasstemp = s.out_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.o_ml[allpasstemp as usize] * constallpass;
                    s.o_ml[s.out_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ml -= 1;
                    if s.out_ml < 0 || s.out_ml > s.delay_m {
                        s.out_ml = s.delay_m;
                    }
                    input_sample_l += s.o_ml[s.out_ml as usize];
                    s.d_ml[5] = s.d_ml[4];
                    s.d_ml[4] = input_sample_l;
                    input_sample_l = (s.d_ml[1] + s.d_ml[2]) / 2.0;
                    allpasstemp = s.out_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.o_nl[allpasstemp as usize] * constallpass;
                    s.o_nl[s.out_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_nl -= 1;
                    if s.out_nl < 0 || s.out_nl > s.delay_n {
                        s.out_nl = s.delay_n;
                    }
                    input_sample_l += s.o_nl[s.out_nl as usize];
                    s.d_nl[5] = s.d_nl[4];
                    s.d_nl[4] = input_sample_l;
                    input_sample_l = (s.d_nl[1] + s.d_nl[2]) / 2.0;
                    allpasstemp = s.out_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.o_ol[allpasstemp as usize] * constallpass;
                    s.o_ol[s.out_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ol -= 1;
                    if s.out_ol < 0 || s.out_ol > s.delay_o {
                        s.out_ol = s.delay_o;
                    }
                    input_sample_l += s.o_ol[s.out_ol as usize];
                    s.d_ol[5] = s.d_ol[4];
                    s.d_ol[4] = input_sample_l;
                    input_sample_l = (s.d_ol[1] + s.d_ol[2]) / 2.0;
                    allpasstemp = s.out_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.o_pl[allpasstemp as usize] * constallpass;
                    s.o_pl[s.out_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_pl -= 1;
                    if s.out_pl < 0 || s.out_pl > s.delay_p {
                        s.out_pl = s.delay_p;
                    }
                    input_sample_l += s.o_pl[s.out_pl as usize];
                    s.d_pl[5] = s.d_pl[4];
                    s.d_pl[4] = input_sample_l;
                    input_sample_l = (s.d_pl[1] + s.d_pl[2]) / 2.0;
                    allpasstemp = s.out_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.o_ql[allpasstemp as usize] * constallpass;
                    s.o_ql[s.out_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ql -= 1;
                    if s.out_ql < 0 || s.out_ql > s.delay_q {
                        s.out_ql = s.delay_q;
                    }
                    input_sample_l += s.o_ql[s.out_ql as usize];
                    s.d_ql[5] = s.d_ql[4];
                    s.d_ql[4] = input_sample_l;
                    input_sample_l = (s.d_ql[1] + s.d_ql[2]) / 2.0;
                    allpasstemp = s.out_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.o_rl[allpasstemp as usize] * constallpass;
                    s.o_rl[s.out_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_rl -= 1;
                    if s.out_rl < 0 || s.out_rl > s.delay_r {
                        s.out_rl = s.delay_r;
                    }
                    input_sample_l += s.o_rl[s.out_rl as usize];
                    s.d_rl[5] = s.d_rl[4];
                    s.d_rl[4] = input_sample_l;
                    input_sample_l = (s.d_rl[1] + s.d_rl[2]) / 2.0;
                    allpasstemp = s.out_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.o_sl[allpasstemp as usize] * constallpass;
                    s.o_sl[s.out_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_sl -= 1;
                    if s.out_sl < 0 || s.out_sl > s.delay_s {
                        s.out_sl = s.delay_s;
                    }
                    input_sample_l += s.o_sl[s.out_sl as usize];
                    s.d_sl[5] = s.d_sl[4];
                    s.d_sl[4] = input_sample_l;
                    input_sample_l = (s.d_sl[1] + s.d_sl[2]) / 2.0;
                    allpasstemp = s.out_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.o_tl[allpasstemp as usize] * constallpass;
                    s.o_tl[s.out_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_tl -= 1;
                    if s.out_tl < 0 || s.out_tl > s.delay_t {
                        s.out_tl = s.delay_t;
                    }
                    input_sample_l += s.o_tl[s.out_tl as usize];
                    s.d_tl[5] = s.d_tl[4];
                    s.d_tl[4] = input_sample_l;
                    input_sample_l = (s.d_tl[1] + s.d_tl[2]) / 2.0;
                    allpasstemp = s.out_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.o_ul[allpasstemp as usize] * constallpass;
                    s.o_ul[s.out_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ul -= 1;
                    if s.out_ul < 0 || s.out_ul > s.delay_u {
                        s.out_ul = s.delay_u;
                    }
                    input_sample_l += s.o_ul[s.out_ul as usize];
                    s.d_ul[5] = s.d_ul[4];
                    s.d_ul[4] = input_sample_l;
                    input_sample_l = (s.d_ul[1] + s.d_ul[2]) / 2.0;
                    allpasstemp = s.out_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.o_vl[allpasstemp as usize] * constallpass;
                    s.o_vl[s.out_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_vl -= 1;
                    if s.out_vl < 0 || s.out_vl > s.delay_v {
                        s.out_vl = s.delay_v;
                    }
                    input_sample_l += s.o_vl[s.out_vl as usize];
                    s.d_vl[5] = s.d_vl[4];
                    s.d_vl[4] = input_sample_l;
                    input_sample_l = (s.d_vl[1] + s.d_vl[2]) / 2.0;
                    allpasstemp = s.out_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.o_wl[allpasstemp as usize] * constallpass;
                    s.o_wl[s.out_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_wl -= 1;
                    if s.out_wl < 0 || s.out_wl > s.delay_w {
                        s.out_wl = s.delay_w;
                    }
                    input_sample_l += s.o_wl[s.out_wl as usize];
                    s.d_wl[5] = s.d_wl[4];
                    s.d_wl[4] = input_sample_l;
                    input_sample_l = (s.d_wl[1] + s.d_wl[2]) / 2.0;
                    allpasstemp = s.out_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.o_xl[allpasstemp as usize] * constallpass;
                    s.o_xl[s.out_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_xl -= 1;
                    if s.out_xl < 0 || s.out_xl > s.delay_x {
                        s.out_xl = s.delay_x;
                    }
                    input_sample_l += s.o_xl[s.out_xl as usize];
                    s.d_xl[5] = s.d_xl[4];
                    s.d_xl[4] = input_sample_l;
                    input_sample_l = (s.d_xl[1] + s.d_xl[2]) / 2.0;
                    allpasstemp = s.out_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.o_yl[allpasstemp as usize] * constallpass;
                    s.o_yl[s.out_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_yl -= 1;
                    if s.out_yl < 0 || s.out_yl > s.delay_y {
                        s.out_yl = s.delay_y;
                    }
                    input_sample_l += s.o_yl[s.out_yl as usize];
                    s.d_yl[5] = s.d_yl[4];
                    s.d_yl[4] = input_sample_l;
                    input_sample_l = (s.d_yl[1] + s.d_yl[2]) / 2.0;
                    allpasstemp = s.out_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.o_zl[allpasstemp as usize] * constallpass;
                    s.o_zl[s.out_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_zl -= 1;
                    if s.out_zl < 0 || s.out_zl > s.delay_z {
                        s.out_zl = s.delay_z;
                    }
                    input_sample_l += s.o_zl[s.out_zl as usize];
                    s.d_zl[5] = s.d_zl[4];
                    s.d_zl[4] = input_sample_l;
                    input_sample_l = s.d_bl[4] * dryness;
                    input_sample_l += s.d_cl[4] * dryness;
                    input_sample_l += s.d_dl[4];
                    input_sample_l += s.d_el[4];
                    input_sample_l += s.d_fl[4];
                    input_sample_l += s.d_gl[4];
                    input_sample_l += s.d_hl[4];
                    input_sample_l += s.d_il[4];
                    input_sample_l += s.d_jl[4];
                    input_sample_l += s.d_kl[4];
                    input_sample_l += s.d_ll[4];
                    input_sample_l += s.d_ml[4];
                    input_sample_l += s.d_nl[4];
                    input_sample_l += s.d_ol[4];
                    input_sample_l += s.d_pl[4];
                    input_sample_l += s.d_ql[4];
                    input_sample_l += s.d_rl[4];
                    input_sample_l += s.d_sl[4];
                    input_sample_l += s.d_tl[4];
                    input_sample_l += s.d_ul[4];
                    input_sample_l += s.d_vl[4];
                    input_sample_l += s.d_wl[4];
                    input_sample_l += s.d_xl[4];
                    input_sample_l += s.d_yl[4];
                    input_sample_l += s.d_zl[4] * wetness;
                    input_sample_l += s.d_bl[5] * dryness;
                    input_sample_l += s.d_cl[5] * dryness;
                    input_sample_l += s.d_dl[5];
                    input_sample_l += s.d_el[5];
                    input_sample_l += s.d_fl[5];
                    input_sample_l += s.d_gl[5];
                    input_sample_l += s.d_hl[5];
                    input_sample_l += s.d_il[5];
                    input_sample_l += s.d_jl[5];
                    input_sample_l += s.d_kl[5];
                    input_sample_l += s.d_ll[5];
                    input_sample_l += s.d_ml[5];
                    input_sample_l += s.d_nl[5];
                    input_sample_l += s.d_ol[5];
                    input_sample_l += s.d_pl[5];
                    input_sample_l += s.d_ql[5];
                    input_sample_l += s.d_rl[5];
                    input_sample_l += s.d_sl[5];
                    input_sample_l += s.d_tl[5];
                    input_sample_l += s.d_ul[5];
                    input_sample_l += s.d_vl[5];
                    input_sample_l += s.d_wl[5];
                    input_sample_l += s.d_xl[5];
                    input_sample_l += s.d_yl[5];
                    input_sample_l += s.d_zl[5] * wetness;
                    input_sample_l /= 26.0 + wetness * 4.0;
                }
                5 => {
                    allpasstemp = s.alp_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.a_al[allpasstemp as usize] * constallpass;
                    s.a_al[s.alp_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_al -= 1;
                    if s.alp_al < 0 || s.alp_al > s.delay_a {
                        s.alp_al = s.delay_a;
                    }
                    input_sample_l += s.a_al[s.alp_al as usize];
                    s.d_al[2] = s.d_al[1];
                    s.d_al[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_al[2]) / 2.0;
                    allpasstemp = s.alp_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.a_bl[allpasstemp as usize] * constallpass;
                    s.a_bl[s.alp_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_bl -= 1;
                    if s.alp_bl < 0 || s.alp_bl > s.delay_b {
                        s.alp_bl = s.delay_b;
                    }
                    input_sample_l += s.a_bl[s.alp_bl as usize];
                    s.d_bl[2] = s.d_bl[1];
                    s.d_bl[1] = input_sample_l;
                    input_sample_l = (s.d_bl[1] + s.d_bl[2]) / 2.0;
                    allpasstemp = s.alp_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.a_cl[allpasstemp as usize] * constallpass;
                    s.a_cl[s.alp_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_cl -= 1;
                    if s.alp_cl < 0 || s.alp_cl > s.delay_c {
                        s.alp_cl = s.delay_c;
                    }
                    input_sample_l += s.a_cl[s.alp_cl as usize];
                    s.d_cl[2] = s.d_cl[1];
                    s.d_cl[1] = input_sample_l;
                    input_sample_l = (s.d_cl[1] + s.d_cl[2]) / 2.0;
                    allpasstemp = s.alp_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.a_dl[allpasstemp as usize] * constallpass;
                    s.a_dl[s.alp_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_dl -= 1;
                    if s.alp_dl < 0 || s.alp_dl > s.delay_d {
                        s.alp_dl = s.delay_d;
                    }
                    input_sample_l += s.a_dl[s.alp_dl as usize];
                    s.d_dl[2] = s.d_dl[1];
                    s.d_dl[1] = input_sample_l;
                    input_sample_l = (s.d_dl[1] + s.d_dl[2]) / 2.0;
                    allpasstemp = s.alp_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.a_el[allpasstemp as usize] * constallpass;
                    s.a_el[s.alp_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_el -= 1;
                    if s.alp_el < 0 || s.alp_el > s.delay_e {
                        s.alp_el = s.delay_e;
                    }
                    input_sample_l += s.a_el[s.alp_el as usize];
                    s.d_el[2] = s.d_el[1];
                    s.d_el[1] = input_sample_l;
                    input_sample_l = (s.d_el[1] + s.d_el[2]) / 2.0;
                    allpasstemp = s.alp_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.a_fl[allpasstemp as usize] * constallpass;
                    s.a_fl[s.alp_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_fl -= 1;
                    if s.alp_fl < 0 || s.alp_fl > s.delay_f {
                        s.alp_fl = s.delay_f;
                    }
                    input_sample_l += s.a_fl[s.alp_fl as usize];
                    s.d_fl[2] = s.d_fl[1];
                    s.d_fl[1] = input_sample_l;
                    input_sample_l = (s.d_fl[1] + s.d_fl[2]) / 2.0;
                    allpasstemp = s.alp_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.a_gl[allpasstemp as usize] * constallpass;
                    s.a_gl[s.alp_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_gl -= 1;
                    if s.alp_gl < 0 || s.alp_gl > s.delay_g {
                        s.alp_gl = s.delay_g;
                    }
                    input_sample_l += s.a_gl[s.alp_gl as usize];
                    s.d_gl[2] = s.d_gl[1];
                    s.d_gl[1] = input_sample_l;
                    input_sample_l = (s.d_gl[1] + s.d_gl[2]) / 2.0;
                    allpasstemp = s.alp_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.a_hl[allpasstemp as usize] * constallpass;
                    s.a_hl[s.alp_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_hl -= 1;
                    if s.alp_hl < 0 || s.alp_hl > s.delay_h {
                        s.alp_hl = s.delay_h;
                    }
                    input_sample_l += s.a_hl[s.alp_hl as usize];
                    s.d_hl[2] = s.d_hl[1];
                    s.d_hl[1] = input_sample_l;
                    input_sample_l = (s.d_hl[1] + s.d_hl[2]) / 2.0;
                    allpasstemp = s.alp_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.a_il[allpasstemp as usize] * constallpass;
                    s.a_il[s.alp_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_il -= 1;
                    if s.alp_il < 0 || s.alp_il > s.delay_i {
                        s.alp_il = s.delay_i;
                    }
                    input_sample_l += s.a_il[s.alp_il as usize];
                    s.d_il[2] = s.d_il[1];
                    s.d_il[1] = input_sample_l;
                    input_sample_l = (s.d_il[1] + s.d_il[2]) / 2.0;
                    allpasstemp = s.alp_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.a_jl[allpasstemp as usize] * constallpass;
                    s.a_jl[s.alp_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_jl -= 1;
                    if s.alp_jl < 0 || s.alp_jl > s.delay_j {
                        s.alp_jl = s.delay_j;
                    }
                    input_sample_l += s.a_jl[s.alp_jl as usize];
                    s.d_jl[2] = s.d_jl[1];
                    s.d_jl[1] = input_sample_l;
                    input_sample_l = (s.d_jl[1] + s.d_jl[2]) / 2.0;
                    allpasstemp = s.alp_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.a_kl[allpasstemp as usize] * constallpass;
                    s.a_kl[s.alp_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_kl -= 1;
                    if s.alp_kl < 0 || s.alp_kl > s.delay_k {
                        s.alp_kl = s.delay_k;
                    }
                    input_sample_l += s.a_kl[s.alp_kl as usize];
                    s.d_kl[2] = s.d_kl[1];
                    s.d_kl[1] = input_sample_l;
                    input_sample_l = (s.d_kl[1] + s.d_kl[2]) / 2.0;
                    allpasstemp = s.alp_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.a_ll[allpasstemp as usize] * constallpass;
                    s.a_ll[s.alp_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ll -= 1;
                    if s.alp_ll < 0 || s.alp_ll > s.delay_l {
                        s.alp_ll = s.delay_l;
                    }
                    input_sample_l += s.a_ll[s.alp_ll as usize];
                    s.d_ll[2] = s.d_ll[1];
                    s.d_ll[1] = input_sample_l;
                    input_sample_l = (s.d_ll[1] + s.d_ll[2]) / 2.0;
                    allpasstemp = s.alp_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.a_ml[allpasstemp as usize] * constallpass;
                    s.a_ml[s.alp_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ml -= 1;
                    if s.alp_ml < 0 || s.alp_ml > s.delay_m {
                        s.alp_ml = s.delay_m;
                    }
                    input_sample_l += s.a_ml[s.alp_ml as usize];
                    s.d_ml[2] = s.d_ml[1];
                    s.d_ml[1] = input_sample_l;
                    input_sample_l = (s.d_ml[1] + s.d_ml[2]) / 2.0;
                    allpasstemp = s.alp_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.a_nl[allpasstemp as usize] * constallpass;
                    s.a_nl[s.alp_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_nl -= 1;
                    if s.alp_nl < 0 || s.alp_nl > s.delay_n {
                        s.alp_nl = s.delay_n;
                    }
                    input_sample_l += s.a_nl[s.alp_nl as usize];
                    s.d_nl[2] = s.d_nl[1];
                    s.d_nl[1] = input_sample_l;
                    input_sample_l = (s.d_nl[1] + s.d_nl[2]) / 2.0;
                    allpasstemp = s.alp_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.a_ol[allpasstemp as usize] * constallpass;
                    s.a_ol[s.alp_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ol -= 1;
                    if s.alp_ol < 0 || s.alp_ol > s.delay_o {
                        s.alp_ol = s.delay_o;
                    }
                    input_sample_l += s.a_ol[s.alp_ol as usize];
                    s.d_ol[2] = s.d_ol[1];
                    s.d_ol[1] = input_sample_l;
                    input_sample_l = (s.d_ol[1] + s.d_ol[2]) / 2.0;
                    allpasstemp = s.alp_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.a_pl[allpasstemp as usize] * constallpass;
                    s.a_pl[s.alp_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_pl -= 1;
                    if s.alp_pl < 0 || s.alp_pl > s.delay_p {
                        s.alp_pl = s.delay_p;
                    }
                    input_sample_l += s.a_pl[s.alp_pl as usize];
                    s.d_pl[2] = s.d_pl[1];
                    s.d_pl[1] = input_sample_l;
                    input_sample_l = (s.d_pl[1] + s.d_pl[2]) / 2.0;
                    allpasstemp = s.alp_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.a_ql[allpasstemp as usize] * constallpass;
                    s.a_ql[s.alp_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ql -= 1;
                    if s.alp_ql < 0 || s.alp_ql > s.delay_q {
                        s.alp_ql = s.delay_q;
                    }
                    input_sample_l += s.a_ql[s.alp_ql as usize];
                    s.d_ql[2] = s.d_ql[1];
                    s.d_ql[1] = input_sample_l;
                    input_sample_l = (s.d_ql[1] + s.d_ql[2]) / 2.0;
                    allpasstemp = s.alp_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.a_rl[allpasstemp as usize] * constallpass;
                    s.a_rl[s.alp_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_rl -= 1;
                    if s.alp_rl < 0 || s.alp_rl > s.delay_r {
                        s.alp_rl = s.delay_r;
                    }
                    input_sample_l += s.a_rl[s.alp_rl as usize];
                    s.d_rl[2] = s.d_rl[1];
                    s.d_rl[1] = input_sample_l;
                    input_sample_l = (s.d_rl[1] + s.d_rl[2]) / 2.0;
                    allpasstemp = s.alp_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.a_sl[allpasstemp as usize] * constallpass;
                    s.a_sl[s.alp_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_sl -= 1;
                    if s.alp_sl < 0 || s.alp_sl > s.delay_s {
                        s.alp_sl = s.delay_s;
                    }
                    input_sample_l += s.a_sl[s.alp_sl as usize];
                    s.d_sl[2] = s.d_sl[1];
                    s.d_sl[1] = input_sample_l;
                    input_sample_l = (s.d_sl[1] + s.d_sl[2]) / 2.0;
                    allpasstemp = s.alp_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.a_tl[allpasstemp as usize] * constallpass;
                    s.a_tl[s.alp_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_tl -= 1;
                    if s.alp_tl < 0 || s.alp_tl > s.delay_t {
                        s.alp_tl = s.delay_t;
                    }
                    input_sample_l += s.a_tl[s.alp_tl as usize];
                    s.d_tl[2] = s.d_tl[1];
                    s.d_tl[1] = input_sample_l;
                    input_sample_l = (s.d_tl[1] + s.d_tl[2]) / 2.0;
                    allpasstemp = s.alp_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.a_ul[allpasstemp as usize] * constallpass;
                    s.a_ul[s.alp_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ul -= 1;
                    if s.alp_ul < 0 || s.alp_ul > s.delay_u {
                        s.alp_ul = s.delay_u;
                    }
                    input_sample_l += s.a_ul[s.alp_ul as usize];
                    s.d_ul[2] = s.d_ul[1];
                    s.d_ul[1] = input_sample_l;
                    input_sample_l = (s.d_ul[1] + s.d_ul[2]) / 2.0;
                    allpasstemp = s.alp_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.a_vl[allpasstemp as usize] * constallpass;
                    s.a_vl[s.alp_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_vl -= 1;
                    if s.alp_vl < 0 || s.alp_vl > s.delay_v {
                        s.alp_vl = s.delay_v;
                    }
                    input_sample_l += s.a_vl[s.alp_vl as usize];
                    s.d_vl[2] = s.d_vl[1];
                    s.d_vl[1] = input_sample_l;
                    input_sample_l = (s.d_vl[1] + s.d_vl[2]) / 2.0;
                    allpasstemp = s.alp_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.a_wl[allpasstemp as usize] * constallpass;
                    s.a_wl[s.alp_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_wl -= 1;
                    if s.alp_wl < 0 || s.alp_wl > s.delay_w {
                        s.alp_wl = s.delay_w;
                    }
                    input_sample_l += s.a_wl[s.alp_wl as usize];
                    s.d_wl[2] = s.d_wl[1];
                    s.d_wl[1] = input_sample_l;
                    input_sample_l = (s.d_wl[1] + s.d_wl[2]) / 2.0;
                    allpasstemp = s.alp_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.a_xl[allpasstemp as usize] * constallpass;
                    s.a_xl[s.alp_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_xl -= 1;
                    if s.alp_xl < 0 || s.alp_xl > s.delay_x {
                        s.alp_xl = s.delay_x;
                    }
                    input_sample_l += s.a_xl[s.alp_xl as usize];
                    s.d_xl[2] = s.d_xl[1];
                    s.d_xl[1] = input_sample_l;
                    input_sample_l = (s.d_xl[1] + s.d_xl[2]) / 2.0;
                    allpasstemp = s.alp_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.a_yl[allpasstemp as usize] * constallpass;
                    s.a_yl[s.alp_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_yl -= 1;
                    if s.alp_yl < 0 || s.alp_yl > s.delay_y {
                        s.alp_yl = s.delay_y;
                    }
                    input_sample_l += s.a_yl[s.alp_yl as usize];
                    s.d_yl[2] = s.d_yl[1];
                    s.d_yl[1] = input_sample_l;
                    input_sample_l = (s.d_yl[1] + s.d_yl[2]) / 2.0;
                    allpasstemp = s.alp_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.a_zl[allpasstemp as usize] * constallpass;
                    s.a_zl[s.alp_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_zl -= 1;
                    if s.alp_zl < 0 || s.alp_zl > s.delay_z {
                        s.alp_zl = s.delay_z;
                    }
                    input_sample_l += s.a_zl[s.alp_zl as usize];
                    s.d_zl[2] = s.d_zl[1];
                    s.d_zl[1] = input_sample_l;
                    input_sample_l = (s.d_zl[1] + s.d_zl[2]) / 2.0;
                    allpasstemp = s.out_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.o_al[allpasstemp as usize] * constallpass;
                    s.o_al[s.out_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_al -= 1;
                    if s.out_al < 0 || s.out_al > s.delay_a {
                        s.out_al = s.delay_a;
                    }
                    input_sample_l += s.o_al[s.out_al as usize];
                    s.d_al[5] = s.d_al[4];
                    s.d_al[4] = input_sample_l;
                    input_sample_l = (s.d_al[4] + s.d_al[5]) / 2.0;
                    allpasstemp = s.out_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.o_bl[allpasstemp as usize] * constallpass;
                    s.o_bl[s.out_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_bl -= 1;
                    if s.out_bl < 0 || s.out_bl > s.delay_b {
                        s.out_bl = s.delay_b;
                    }
                    input_sample_l += s.o_bl[s.out_bl as usize];
                    s.d_bl[5] = s.d_bl[4];
                    s.d_bl[4] = input_sample_l;
                    input_sample_l = (s.d_bl[4] + s.d_bl[5]) / 2.0;
                    allpasstemp = s.out_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.o_cl[allpasstemp as usize] * constallpass;
                    s.o_cl[s.out_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_cl -= 1;
                    if s.out_cl < 0 || s.out_cl > s.delay_c {
                        s.out_cl = s.delay_c;
                    }
                    input_sample_l += s.o_cl[s.out_cl as usize];
                    s.d_cl[5] = s.d_cl[4];
                    s.d_cl[4] = input_sample_l;
                    input_sample_l = (s.d_cl[4] + s.d_cl[5]) / 2.0;
                    allpasstemp = s.out_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.o_dl[allpasstemp as usize] * constallpass;
                    s.o_dl[s.out_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_dl -= 1;
                    if s.out_dl < 0 || s.out_dl > s.delay_d {
                        s.out_dl = s.delay_d;
                    }
                    input_sample_l += s.o_dl[s.out_dl as usize];
                    s.d_dl[5] = s.d_dl[4];
                    s.d_dl[4] = input_sample_l;
                    input_sample_l = (s.d_dl[4] + s.d_dl[5]) / 2.0;
                    allpasstemp = s.out_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.o_el[allpasstemp as usize] * constallpass;
                    s.o_el[s.out_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_el -= 1;
                    if s.out_el < 0 || s.out_el > s.delay_e {
                        s.out_el = s.delay_e;
                    }
                    input_sample_l += s.o_el[s.out_el as usize];
                    s.d_el[5] = s.d_el[4];
                    s.d_el[4] = input_sample_l;
                    input_sample_l = (s.d_el[4] + s.d_el[5]) / 2.0;
                    allpasstemp = s.out_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.o_fl[allpasstemp as usize] * constallpass;
                    s.o_fl[s.out_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_fl -= 1;
                    if s.out_fl < 0 || s.out_fl > s.delay_f {
                        s.out_fl = s.delay_f;
                    }
                    input_sample_l += s.o_fl[s.out_fl as usize];
                    s.d_fl[5] = s.d_fl[4];
                    s.d_fl[4] = input_sample_l;
                    input_sample_l = (s.d_fl[4] + s.d_fl[5]) / 2.0;
                    allpasstemp = s.out_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.o_gl[allpasstemp as usize] * constallpass;
                    s.o_gl[s.out_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_gl -= 1;
                    if s.out_gl < 0 || s.out_gl > s.delay_g {
                        s.out_gl = s.delay_g;
                    }
                    input_sample_l += s.o_gl[s.out_gl as usize];
                    s.d_gl[5] = s.d_gl[4];
                    s.d_gl[4] = input_sample_l;
                    input_sample_l = (s.d_gl[4] + s.d_gl[5]) / 2.0;
                    allpasstemp = s.out_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.o_hl[allpasstemp as usize] * constallpass;
                    s.o_hl[s.out_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_hl -= 1;
                    if s.out_hl < 0 || s.out_hl > s.delay_h {
                        s.out_hl = s.delay_h;
                    }
                    input_sample_l += s.o_hl[s.out_hl as usize];
                    s.d_hl[5] = s.d_hl[4];
                    s.d_hl[4] = input_sample_l;
                    input_sample_l = (s.d_hl[4] + s.d_hl[5]) / 2.0;
                    allpasstemp = s.out_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.o_il[allpasstemp as usize] * constallpass;
                    s.o_il[s.out_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_il -= 1;
                    if s.out_il < 0 || s.out_il > s.delay_i {
                        s.out_il = s.delay_i;
                    }
                    input_sample_l += s.o_il[s.out_il as usize];
                    s.d_il[5] = s.d_il[4];
                    s.d_il[4] = input_sample_l;
                    input_sample_l = (s.d_il[4] + s.d_il[5]) / 2.0;
                    allpasstemp = s.out_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.o_jl[allpasstemp as usize] * constallpass;
                    s.o_jl[s.out_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_jl -= 1;
                    if s.out_jl < 0 || s.out_jl > s.delay_j {
                        s.out_jl = s.delay_j;
                    }
                    input_sample_l += s.o_jl[s.out_jl as usize];
                    s.d_jl[5] = s.d_jl[4];
                    s.d_jl[4] = input_sample_l;
                    input_sample_l = (s.d_jl[4] + s.d_jl[5]) / 2.0;
                    allpasstemp = s.out_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.o_kl[allpasstemp as usize] * constallpass;
                    s.o_kl[s.out_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_kl -= 1;
                    if s.out_kl < 0 || s.out_kl > s.delay_k {
                        s.out_kl = s.delay_k;
                    }
                    input_sample_l += s.o_kl[s.out_kl as usize];
                    s.d_kl[5] = s.d_kl[4];
                    s.d_kl[4] = input_sample_l;
                    input_sample_l = (s.d_kl[4] + s.d_kl[5]) / 2.0;
                    allpasstemp = s.out_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.o_ll[allpasstemp as usize] * constallpass;
                    s.o_ll[s.out_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ll -= 1;
                    if s.out_ll < 0 || s.out_ll > s.delay_l {
                        s.out_ll = s.delay_l;
                    }
                    input_sample_l += s.o_ll[s.out_ll as usize];
                    s.d_ll[5] = s.d_ll[4];
                    s.d_ll[4] = input_sample_l;
                    input_sample_l = (s.d_ll[4] + s.d_ll[5]) / 2.0;
                    allpasstemp = s.out_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.o_ml[allpasstemp as usize] * constallpass;
                    s.o_ml[s.out_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ml -= 1;
                    if s.out_ml < 0 || s.out_ml > s.delay_m {
                        s.out_ml = s.delay_m;
                    }
                    input_sample_l += s.o_ml[s.out_ml as usize];
                    s.d_ml[5] = s.d_ml[4];
                    s.d_ml[4] = input_sample_l;
                    input_sample_l = (s.d_ml[4] + s.d_ml[5]) / 2.0;
                    allpasstemp = s.out_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.o_nl[allpasstemp as usize] * constallpass;
                    s.o_nl[s.out_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_nl -= 1;
                    if s.out_nl < 0 || s.out_nl > s.delay_n {
                        s.out_nl = s.delay_n;
                    }
                    input_sample_l += s.o_nl[s.out_nl as usize];
                    s.d_nl[5] = s.d_nl[4];
                    s.d_nl[4] = input_sample_l;
                    input_sample_l = (s.d_nl[4] + s.d_nl[5]) / 2.0;
                    allpasstemp = s.out_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.o_ol[allpasstemp as usize] * constallpass;
                    s.o_ol[s.out_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ol -= 1;
                    if s.out_ol < 0 || s.out_ol > s.delay_o {
                        s.out_ol = s.delay_o;
                    }
                    input_sample_l += s.o_ol[s.out_ol as usize];
                    s.d_ol[5] = s.d_ol[4];
                    s.d_ol[4] = input_sample_l;
                    input_sample_l = (s.d_ol[4] + s.d_ol[5]) / 2.0;
                    allpasstemp = s.out_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.o_pl[allpasstemp as usize] * constallpass;
                    s.o_pl[s.out_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_pl -= 1;
                    if s.out_pl < 0 || s.out_pl > s.delay_p {
                        s.out_pl = s.delay_p;
                    }
                    input_sample_l += s.o_pl[s.out_pl as usize];
                    s.d_pl[5] = s.d_pl[4];
                    s.d_pl[4] = input_sample_l;
                    input_sample_l = (s.d_pl[4] + s.d_pl[5]) / 2.0;
                    allpasstemp = s.out_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.o_ql[allpasstemp as usize] * constallpass;
                    s.o_ql[s.out_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ql -= 1;
                    if s.out_ql < 0 || s.out_ql > s.delay_q {
                        s.out_ql = s.delay_q;
                    }
                    input_sample_l += s.o_ql[s.out_ql as usize];
                    s.d_ql[5] = s.d_ql[4];
                    s.d_ql[4] = input_sample_l;
                    input_sample_l = (s.d_ql[4] + s.d_ql[5]) / 2.0;
                    allpasstemp = s.out_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.o_rl[allpasstemp as usize] * constallpass;
                    s.o_rl[s.out_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_rl -= 1;
                    if s.out_rl < 0 || s.out_rl > s.delay_r {
                        s.out_rl = s.delay_r;
                    }
                    input_sample_l += s.o_rl[s.out_rl as usize];
                    s.d_rl[5] = s.d_rl[4];
                    s.d_rl[4] = input_sample_l;
                    input_sample_l = (s.d_rl[4] + s.d_rl[5]) / 2.0;
                    allpasstemp = s.out_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.o_sl[allpasstemp as usize] * constallpass;
                    s.o_sl[s.out_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_sl -= 1;
                    if s.out_sl < 0 || s.out_sl > s.delay_s {
                        s.out_sl = s.delay_s;
                    }
                    input_sample_l += s.o_sl[s.out_sl as usize];
                    s.d_sl[5] = s.d_sl[4];
                    s.d_sl[4] = input_sample_l;
                    input_sample_l = (s.d_sl[4] + s.d_sl[5]) / 2.0;
                    allpasstemp = s.out_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.o_tl[allpasstemp as usize] * constallpass;
                    s.o_tl[s.out_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_tl -= 1;
                    if s.out_tl < 0 || s.out_tl > s.delay_t {
                        s.out_tl = s.delay_t;
                    }
                    input_sample_l += s.o_tl[s.out_tl as usize];
                    s.d_tl[5] = s.d_tl[4];
                    s.d_tl[4] = input_sample_l;
                    input_sample_l = (s.d_tl[4] + s.d_tl[5]) / 2.0;
                    allpasstemp = s.out_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.o_ul[allpasstemp as usize] * constallpass;
                    s.o_ul[s.out_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ul -= 1;
                    if s.out_ul < 0 || s.out_ul > s.delay_u {
                        s.out_ul = s.delay_u;
                    }
                    input_sample_l += s.o_ul[s.out_ul as usize];
                    s.d_ul[5] = s.d_ul[4];
                    s.d_ul[4] = input_sample_l;
                    input_sample_l = (s.d_ul[4] + s.d_ul[5]) / 2.0;
                    allpasstemp = s.out_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.o_vl[allpasstemp as usize] * constallpass;
                    s.o_vl[s.out_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_vl -= 1;
                    if s.out_vl < 0 || s.out_vl > s.delay_v {
                        s.out_vl = s.delay_v;
                    }
                    input_sample_l += s.o_vl[s.out_vl as usize];
                    s.d_vl[5] = s.d_vl[4];
                    s.d_vl[4] = input_sample_l;
                    input_sample_l = (s.d_vl[4] + s.d_vl[5]) / 2.0;
                    allpasstemp = s.out_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.o_wl[allpasstemp as usize] * constallpass;
                    s.o_wl[s.out_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_wl -= 1;
                    if s.out_wl < 0 || s.out_wl > s.delay_w {
                        s.out_wl = s.delay_w;
                    }
                    input_sample_l += s.o_wl[s.out_wl as usize];
                    s.d_wl[5] = s.d_wl[4];
                    s.d_wl[4] = input_sample_l;
                    input_sample_l = (s.d_wl[4] + s.d_wl[5]) / 2.0;
                    allpasstemp = s.out_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.o_xl[allpasstemp as usize] * constallpass;
                    s.o_xl[s.out_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_xl -= 1;
                    if s.out_xl < 0 || s.out_xl > s.delay_x {
                        s.out_xl = s.delay_x;
                    }
                    input_sample_l += s.o_xl[s.out_xl as usize];
                    s.d_xl[5] = s.d_xl[4];
                    s.d_xl[4] = input_sample_l;
                    input_sample_l = (s.d_xl[4] + s.d_xl[5]) / 2.0;
                    allpasstemp = s.out_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.o_yl[allpasstemp as usize] * constallpass;
                    s.o_yl[s.out_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_yl -= 1;
                    if s.out_yl < 0 || s.out_yl > s.delay_y {
                        s.out_yl = s.delay_y;
                    }
                    input_sample_l += s.o_yl[s.out_yl as usize];
                    s.d_yl[5] = s.d_yl[4];
                    s.d_yl[4] = input_sample_l;
                    input_sample_l = (s.d_yl[4] + s.d_yl[5]) / 2.0;
                    allpasstemp = s.out_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.o_zl[allpasstemp as usize] * constallpass;
                    s.o_zl[s.out_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_zl -= 1;
                    if s.out_zl < 0 || s.out_zl > s.delay_z {
                        s.out_zl = s.delay_z;
                    }
                    input_sample_l += s.o_zl[s.out_zl as usize];
                    s.d_zl[5] = s.d_zl[4];
                    s.d_zl[4] = input_sample_l;
                    input_sample_l = (s.d_zl[4] + s.d_zl[5]) / 2.0;
                }
                6 => {
                    allpasstemp = s.alp_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.a_al[allpasstemp as usize] * constallpass;
                    s.a_al[s.alp_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_al -= 1;
                    if s.alp_al < 0 || s.alp_al > s.delay_a {
                        s.alp_al = s.delay_a;
                    }
                    input_sample_l += s.a_al[s.alp_al as usize];
                    s.d_al[3] = s.d_al[2];
                    s.d_al[2] = s.d_al[1];
                    s.d_al[1] = input_sample_l;
                    input_sample_l = (s.d_al[1] + s.d_al[2] + s.d_zl[3]) / 3.0;
                    allpasstemp = s.alp_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.a_bl[allpasstemp as usize] * constallpass;
                    s.a_bl[s.alp_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_bl -= 1;
                    if s.alp_bl < 0 || s.alp_bl > s.delay_b {
                        s.alp_bl = s.delay_b;
                    }
                    input_sample_l += s.a_bl[s.alp_bl as usize];
                    s.d_bl[3] = s.d_bl[2];
                    s.d_bl[2] = s.d_bl[1];
                    s.d_bl[1] = input_sample_l;
                    input_sample_l = (s.d_bl[1] + s.d_bl[2] + s.d_bl[3]) / 3.0;
                    allpasstemp = s.alp_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.a_cl[allpasstemp as usize] * constallpass;
                    s.a_cl[s.alp_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_cl -= 1;
                    if s.alp_cl < 0 || s.alp_cl > s.delay_c {
                        s.alp_cl = s.delay_c;
                    }
                    input_sample_l += s.a_cl[s.alp_cl as usize];
                    s.d_cl[3] = s.d_cl[2];
                    s.d_cl[2] = s.d_cl[1];
                    s.d_cl[1] = input_sample_l;
                    input_sample_l = (s.d_cl[1] + s.d_cl[2] + s.d_cl[3]) / 3.0;
                    allpasstemp = s.alp_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.a_dl[allpasstemp as usize] * constallpass;
                    s.a_dl[s.alp_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_dl -= 1;
                    if s.alp_dl < 0 || s.alp_dl > s.delay_d {
                        s.alp_dl = s.delay_d;
                    }
                    input_sample_l += s.a_dl[s.alp_dl as usize];
                    s.d_dl[3] = s.d_dl[2];
                    s.d_dl[2] = s.d_dl[1];
                    s.d_dl[1] = input_sample_l;
                    input_sample_l = (s.d_dl[1] + s.d_dl[2] + s.d_dl[3]) / 3.0;
                    allpasstemp = s.alp_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.a_el[allpasstemp as usize] * constallpass;
                    s.a_el[s.alp_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_el -= 1;
                    if s.alp_el < 0 || s.alp_el > s.delay_e {
                        s.alp_el = s.delay_e;
                    }
                    input_sample_l += s.a_el[s.alp_el as usize];
                    s.d_el[3] = s.d_el[2];
                    s.d_el[2] = s.d_el[1];
                    s.d_el[1] = input_sample_l;
                    input_sample_l = (s.d_el[1] + s.d_el[2] + s.d_el[3]) / 3.0;
                    allpasstemp = s.alp_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.a_fl[allpasstemp as usize] * constallpass;
                    s.a_fl[s.alp_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_fl -= 1;
                    if s.alp_fl < 0 || s.alp_fl > s.delay_f {
                        s.alp_fl = s.delay_f;
                    }
                    input_sample_l += s.a_fl[s.alp_fl as usize];
                    s.d_fl[3] = s.d_fl[2];
                    s.d_fl[2] = s.d_fl[1];
                    s.d_fl[1] = input_sample_l;
                    input_sample_l = (s.d_fl[1] + s.d_fl[2] + s.d_fl[3]) / 3.0;
                    allpasstemp = s.alp_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.a_gl[allpasstemp as usize] * constallpass;
                    s.a_gl[s.alp_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_gl -= 1;
                    if s.alp_gl < 0 || s.alp_gl > s.delay_g {
                        s.alp_gl = s.delay_g;
                    }
                    input_sample_l += s.a_gl[s.alp_gl as usize];
                    s.d_gl[3] = s.d_gl[2];
                    s.d_gl[2] = s.d_gl[1];
                    s.d_gl[1] = input_sample_l;
                    input_sample_l = (s.d_gl[1] + s.d_gl[2] + s.d_gl[3]) / 3.0;
                    allpasstemp = s.alp_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.a_hl[allpasstemp as usize] * constallpass;
                    s.a_hl[s.alp_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_hl -= 1;
                    if s.alp_hl < 0 || s.alp_hl > s.delay_h {
                        s.alp_hl = s.delay_h;
                    }
                    input_sample_l += s.a_hl[s.alp_hl as usize];
                    s.d_hl[3] = s.d_hl[2];
                    s.d_hl[2] = s.d_hl[1];
                    s.d_hl[1] = input_sample_l;
                    input_sample_l = (s.d_hl[1] + s.d_hl[2] + s.d_hl[3]) / 3.0;
                    allpasstemp = s.alp_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.a_il[allpasstemp as usize] * constallpass;
                    s.a_il[s.alp_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_il -= 1;
                    if s.alp_il < 0 || s.alp_il > s.delay_i {
                        s.alp_il = s.delay_i;
                    }
                    input_sample_l += s.a_il[s.alp_il as usize];
                    s.d_il[3] = s.d_il[2];
                    s.d_il[2] = s.d_il[1];
                    s.d_il[1] = input_sample_l;
                    input_sample_l = (s.d_il[1] + s.d_il[2] + s.d_il[3]) / 3.0;
                    allpasstemp = s.alp_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.a_jl[allpasstemp as usize] * constallpass;
                    s.a_jl[s.alp_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_jl -= 1;
                    if s.alp_jl < 0 || s.alp_jl > s.delay_j {
                        s.alp_jl = s.delay_j;
                    }
                    input_sample_l += s.a_jl[s.alp_jl as usize];
                    s.d_jl[3] = s.d_jl[2];
                    s.d_jl[2] = s.d_jl[1];
                    s.d_jl[1] = input_sample_l;
                    input_sample_l = (s.d_jl[1] + s.d_jl[2] + s.d_jl[3]) / 3.0;
                    allpasstemp = s.alp_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.a_kl[allpasstemp as usize] * constallpass;
                    s.a_kl[s.alp_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_kl -= 1;
                    if s.alp_kl < 0 || s.alp_kl > s.delay_k {
                        s.alp_kl = s.delay_k;
                    }
                    input_sample_l += s.a_kl[s.alp_kl as usize];
                    s.d_kl[3] = s.d_kl[2];
                    s.d_kl[2] = s.d_kl[1];
                    s.d_kl[1] = input_sample_l;
                    input_sample_l = (s.d_kl[1] + s.d_kl[2] + s.d_kl[3]) / 3.0;
                    allpasstemp = s.alp_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.a_ll[allpasstemp as usize] * constallpass;
                    s.a_ll[s.alp_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ll -= 1;
                    if s.alp_ll < 0 || s.alp_ll > s.delay_l {
                        s.alp_ll = s.delay_l;
                    }
                    input_sample_l += s.a_ll[s.alp_ll as usize];
                    s.d_ll[3] = s.d_ll[2];
                    s.d_ll[2] = s.d_ll[1];
                    s.d_ll[1] = input_sample_l;
                    input_sample_l = (s.d_ll[1] + s.d_ll[2] + s.d_ll[3]) / 3.0;
                    allpasstemp = s.alp_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.a_ml[allpasstemp as usize] * constallpass;
                    s.a_ml[s.alp_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ml -= 1;
                    if s.alp_ml < 0 || s.alp_ml > s.delay_m {
                        s.alp_ml = s.delay_m;
                    }
                    input_sample_l += s.a_ml[s.alp_ml as usize];
                    s.d_ml[3] = s.d_ml[2];
                    s.d_ml[2] = s.d_ml[1];
                    s.d_ml[1] = input_sample_l;
                    input_sample_l = (s.d_ml[1] + s.d_ml[2] + s.d_ml[3]) / 3.0;
                    allpasstemp = s.alp_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.a_nl[allpasstemp as usize] * constallpass;
                    s.a_nl[s.alp_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_nl -= 1;
                    if s.alp_nl < 0 || s.alp_nl > s.delay_n {
                        s.alp_nl = s.delay_n;
                    }
                    input_sample_l += s.a_nl[s.alp_nl as usize];
                    s.d_nl[3] = s.d_nl[2];
                    s.d_nl[2] = s.d_nl[1];
                    s.d_nl[1] = input_sample_l;
                    input_sample_l = (s.d_nl[1] + s.d_nl[2] + s.d_nl[3]) / 3.0;
                    allpasstemp = s.alp_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.a_ol[allpasstemp as usize] * constallpass;
                    s.a_ol[s.alp_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ol -= 1;
                    if s.alp_ol < 0 || s.alp_ol > s.delay_o {
                        s.alp_ol = s.delay_o;
                    }
                    input_sample_l += s.a_ol[s.alp_ol as usize];
                    s.d_ol[3] = s.d_ol[2];
                    s.d_ol[2] = s.d_ol[1];
                    s.d_ol[1] = input_sample_l;
                    input_sample_l = (s.d_ol[1] + s.d_ol[2] + s.d_ol[3]) / 3.0;
                    allpasstemp = s.alp_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.a_pl[allpasstemp as usize] * constallpass;
                    s.a_pl[s.alp_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_pl -= 1;
                    if s.alp_pl < 0 || s.alp_pl > s.delay_p {
                        s.alp_pl = s.delay_p;
                    }
                    input_sample_l += s.a_pl[s.alp_pl as usize];
                    s.d_pl[3] = s.d_pl[2];
                    s.d_pl[2] = s.d_pl[1];
                    s.d_pl[1] = input_sample_l;
                    input_sample_l = (s.d_pl[1] + s.d_pl[2] + s.d_pl[3]) / 3.0;
                    allpasstemp = s.alp_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.a_ql[allpasstemp as usize] * constallpass;
                    s.a_ql[s.alp_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ql -= 1;
                    if s.alp_ql < 0 || s.alp_ql > s.delay_q {
                        s.alp_ql = s.delay_q;
                    }
                    input_sample_l += s.a_ql[s.alp_ql as usize];
                    s.d_ql[3] = s.d_ql[2];
                    s.d_ql[2] = s.d_ql[1];
                    s.d_ql[1] = input_sample_l;
                    input_sample_l = (s.d_ql[1] + s.d_ql[2] + s.d_ql[3]) / 3.0;
                    allpasstemp = s.alp_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.a_rl[allpasstemp as usize] * constallpass;
                    s.a_rl[s.alp_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_rl -= 1;
                    if s.alp_rl < 0 || s.alp_rl > s.delay_r {
                        s.alp_rl = s.delay_r;
                    }
                    input_sample_l += s.a_rl[s.alp_rl as usize];
                    s.d_rl[3] = s.d_rl[2];
                    s.d_rl[2] = s.d_rl[1];
                    s.d_rl[1] = input_sample_l;
                    input_sample_l = (s.d_rl[1] + s.d_rl[2] + s.d_rl[3]) / 3.0;
                    allpasstemp = s.alp_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.a_sl[allpasstemp as usize] * constallpass;
                    s.a_sl[s.alp_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_sl -= 1;
                    if s.alp_sl < 0 || s.alp_sl > s.delay_s {
                        s.alp_sl = s.delay_s;
                    }
                    input_sample_l += s.a_sl[s.alp_sl as usize];
                    s.d_sl[3] = s.d_sl[2];
                    s.d_sl[2] = s.d_sl[1];
                    s.d_sl[1] = input_sample_l;
                    input_sample_l = (s.d_sl[1] + s.d_sl[2] + s.d_sl[3]) / 3.0;
                    allpasstemp = s.alp_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.a_tl[allpasstemp as usize] * constallpass;
                    s.a_tl[s.alp_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_tl -= 1;
                    if s.alp_tl < 0 || s.alp_tl > s.delay_t {
                        s.alp_tl = s.delay_t;
                    }
                    input_sample_l += s.a_tl[s.alp_tl as usize];
                    s.d_tl[3] = s.d_tl[2];
                    s.d_tl[2] = s.d_tl[1];
                    s.d_tl[1] = input_sample_l;
                    input_sample_l = (s.d_tl[1] + s.d_tl[2] + s.d_tl[3]) / 3.0;
                    allpasstemp = s.alp_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.a_ul[allpasstemp as usize] * constallpass;
                    s.a_ul[s.alp_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_ul -= 1;
                    if s.alp_ul < 0 || s.alp_ul > s.delay_u {
                        s.alp_ul = s.delay_u;
                    }
                    input_sample_l += s.a_ul[s.alp_ul as usize];
                    s.d_ul[3] = s.d_ul[2];
                    s.d_ul[2] = s.d_ul[1];
                    s.d_ul[1] = input_sample_l;
                    input_sample_l = (s.d_ul[1] + s.d_ul[2] + s.d_ul[3]) / 3.0;
                    allpasstemp = s.alp_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.a_vl[allpasstemp as usize] * constallpass;
                    s.a_vl[s.alp_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_vl -= 1;
                    if s.alp_vl < 0 || s.alp_vl > s.delay_v {
                        s.alp_vl = s.delay_v;
                    }
                    input_sample_l += s.a_vl[s.alp_vl as usize];
                    s.d_vl[3] = s.d_vl[2];
                    s.d_vl[2] = s.d_vl[1];
                    s.d_vl[1] = input_sample_l;
                    input_sample_l = (s.d_vl[1] + s.d_vl[2] + s.d_vl[3]) / 3.0;
                    allpasstemp = s.alp_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.a_wl[allpasstemp as usize] * constallpass;
                    s.a_wl[s.alp_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_wl -= 1;
                    if s.alp_wl < 0 || s.alp_wl > s.delay_w {
                        s.alp_wl = s.delay_w;
                    }
                    input_sample_l += s.a_wl[s.alp_wl as usize];
                    s.d_wl[3] = s.d_wl[2];
                    s.d_wl[2] = s.d_wl[1];
                    s.d_wl[1] = input_sample_l;
                    input_sample_l = (s.d_wl[1] + s.d_wl[2] + s.d_wl[3]) / 3.0;
                    allpasstemp = s.alp_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.a_xl[allpasstemp as usize] * constallpass;
                    s.a_xl[s.alp_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_xl -= 1;
                    if s.alp_xl < 0 || s.alp_xl > s.delay_x {
                        s.alp_xl = s.delay_x;
                    }
                    input_sample_l += s.a_xl[s.alp_xl as usize];
                    s.d_xl[3] = s.d_xl[2];
                    s.d_xl[2] = s.d_xl[1];
                    s.d_xl[1] = input_sample_l;
                    input_sample_l = (s.d_xl[1] + s.d_xl[2] + s.d_xl[3]) / 3.0;
                    allpasstemp = s.alp_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.a_yl[allpasstemp as usize] * constallpass;
                    s.a_yl[s.alp_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_yl -= 1;
                    if s.alp_yl < 0 || s.alp_yl > s.delay_y {
                        s.alp_yl = s.delay_y;
                    }
                    input_sample_l += s.a_yl[s.alp_yl as usize];
                    s.d_yl[3] = s.d_yl[2];
                    s.d_yl[2] = s.d_yl[1];
                    s.d_yl[1] = input_sample_l;
                    input_sample_l = (s.d_yl[1] + s.d_yl[2] + s.d_yl[3]) / 3.0;
                    allpasstemp = s.alp_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.a_zl[allpasstemp as usize] * constallpass;
                    s.a_zl[s.alp_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.alp_zl -= 1;
                    if s.alp_zl < 0 || s.alp_zl > s.delay_z {
                        s.alp_zl = s.delay_z;
                    }
                    input_sample_l += s.a_zl[s.alp_zl as usize];
                    s.d_zl[3] = s.d_zl[2];
                    s.d_zl[2] = s.d_zl[1];
                    s.d_zl[1] = input_sample_l;
                    input_sample_l = (s.d_zl[1] + s.d_zl[2] + s.d_zl[3]) / 3.0;
                    allpasstemp = s.out_al - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_l -= s.o_al[allpasstemp as usize] * constallpass;
                    s.o_al[s.out_al as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_al -= 1;
                    if s.out_al < 0 || s.out_al > s.delay_a {
                        s.out_al = s.delay_a;
                    }
                    input_sample_l += s.o_al[s.out_al as usize];
                    s.d_al[6] = s.d_al[5];
                    s.d_al[5] = s.d_al[4];
                    s.d_al[4] = input_sample_l;
                    input_sample_l = (s.d_cl[1] + s.d_al[5] + s.d_al[6]) / 3.0;
                    allpasstemp = s.out_bl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_l -= s.o_bl[allpasstemp as usize] * constallpass;
                    s.o_bl[s.out_bl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_bl -= 1;
                    if s.out_bl < 0 || s.out_bl > s.delay_b {
                        s.out_bl = s.delay_b;
                    }
                    input_sample_l += s.o_bl[s.out_bl as usize];
                    s.d_bl[6] = s.d_bl[5];
                    s.d_bl[5] = s.d_bl[4];
                    s.d_bl[4] = input_sample_l;
                    input_sample_l = (s.d_bl[4] + s.d_bl[5] + s.d_bl[6]) / 3.0;
                    allpasstemp = s.out_cl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_l -= s.o_cl[allpasstemp as usize] * constallpass;
                    s.o_cl[s.out_cl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_cl -= 1;
                    if s.out_cl < 0 || s.out_cl > s.delay_c {
                        s.out_cl = s.delay_c;
                    }
                    input_sample_l += s.o_cl[s.out_cl as usize];
                    s.d_cl[6] = s.d_cl[5];
                    s.d_cl[5] = s.d_cl[4];
                    s.d_cl[4] = input_sample_l;
                    input_sample_l = (s.d_cl[4] + s.d_cl[5] + s.d_cl[6]) / 3.0;
                    allpasstemp = s.out_dl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_l -= s.o_dl[allpasstemp as usize] * constallpass;
                    s.o_dl[s.out_dl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_dl -= 1;
                    if s.out_dl < 0 || s.out_dl > s.delay_d {
                        s.out_dl = s.delay_d;
                    }
                    input_sample_l += s.o_dl[s.out_dl as usize];
                    s.d_dl[6] = s.d_dl[5];
                    s.d_dl[5] = s.d_dl[4];
                    s.d_dl[4] = input_sample_l;
                    input_sample_l = (s.d_dl[4] + s.d_dl[5] + s.d_dl[6]) / 3.0;
                    allpasstemp = s.out_el - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_l -= s.o_el[allpasstemp as usize] * constallpass;
                    s.o_el[s.out_el as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_el -= 1;
                    if s.out_el < 0 || s.out_el > s.delay_e {
                        s.out_el = s.delay_e;
                    }
                    input_sample_l += s.o_el[s.out_el as usize];
                    s.d_el[6] = s.d_el[5];
                    s.d_el[5] = s.d_el[4];
                    s.d_el[4] = input_sample_l;
                    input_sample_l = (s.d_el[4] + s.d_el[5] + s.d_el[6]) / 3.0;
                    allpasstemp = s.out_fl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_l -= s.o_fl[allpasstemp as usize] * constallpass;
                    s.o_fl[s.out_fl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_fl -= 1;
                    if s.out_fl < 0 || s.out_fl > s.delay_f {
                        s.out_fl = s.delay_f;
                    }
                    input_sample_l += s.o_fl[s.out_fl as usize];
                    s.d_fl[6] = s.d_fl[5];
                    s.d_fl[5] = s.d_fl[4];
                    s.d_fl[4] = input_sample_l;
                    input_sample_l = (s.d_fl[4] + s.d_fl[5] + s.d_fl[6]) / 3.0;
                    allpasstemp = s.out_gl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_l -= s.o_gl[allpasstemp as usize] * constallpass;
                    s.o_gl[s.out_gl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_gl -= 1;
                    if s.out_gl < 0 || s.out_gl > s.delay_g {
                        s.out_gl = s.delay_g;
                    }
                    input_sample_l += s.o_gl[s.out_gl as usize];
                    s.d_gl[6] = s.d_gl[5];
                    s.d_gl[5] = s.d_gl[4];
                    s.d_gl[4] = input_sample_l;
                    input_sample_l = (s.d_gl[4] + s.d_gl[5] + s.d_gl[6]) / 3.0;
                    allpasstemp = s.out_hl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_l -= s.o_hl[allpasstemp as usize] * constallpass;
                    s.o_hl[s.out_hl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_hl -= 1;
                    if s.out_hl < 0 || s.out_hl > s.delay_h {
                        s.out_hl = s.delay_h;
                    }
                    input_sample_l += s.o_hl[s.out_hl as usize];
                    s.d_hl[6] = s.d_hl[5];
                    s.d_hl[5] = s.d_hl[4];
                    s.d_hl[4] = input_sample_l;
                    input_sample_l = (s.d_hl[4] + s.d_hl[5] + s.d_hl[6]) / 3.0;
                    allpasstemp = s.out_il - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_l -= s.o_il[allpasstemp as usize] * constallpass;
                    s.o_il[s.out_il as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_il -= 1;
                    if s.out_il < 0 || s.out_il > s.delay_i {
                        s.out_il = s.delay_i;
                    }
                    input_sample_l += s.o_il[s.out_il as usize];
                    s.d_il[6] = s.d_il[5];
                    s.d_il[5] = s.d_il[4];
                    s.d_il[4] = input_sample_l;
                    input_sample_l = (s.d_il[4] + s.d_il[5] + s.d_il[6]) / 3.0;
                    allpasstemp = s.out_jl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_l -= s.o_jl[allpasstemp as usize] * constallpass;
                    s.o_jl[s.out_jl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_jl -= 1;
                    if s.out_jl < 0 || s.out_jl > s.delay_j {
                        s.out_jl = s.delay_j;
                    }
                    input_sample_l += s.o_jl[s.out_jl as usize];
                    s.d_jl[6] = s.d_jl[5];
                    s.d_jl[5] = s.d_jl[4];
                    s.d_jl[4] = input_sample_l;
                    input_sample_l = (s.d_jl[4] + s.d_jl[5] + s.d_jl[6]) / 3.0;
                    allpasstemp = s.out_kl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_l -= s.o_kl[allpasstemp as usize] * constallpass;
                    s.o_kl[s.out_kl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_kl -= 1;
                    if s.out_kl < 0 || s.out_kl > s.delay_k {
                        s.out_kl = s.delay_k;
                    }
                    input_sample_l += s.o_kl[s.out_kl as usize];
                    s.d_kl[6] = s.d_kl[5];
                    s.d_kl[5] = s.d_kl[4];
                    s.d_kl[4] = input_sample_l;
                    input_sample_l = (s.d_kl[4] + s.d_kl[5] + s.d_kl[6]) / 3.0;
                    allpasstemp = s.out_ll - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_l -= s.o_ll[allpasstemp as usize] * constallpass;
                    s.o_ll[s.out_ll as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ll -= 1;
                    if s.out_ll < 0 || s.out_ll > s.delay_l {
                        s.out_ll = s.delay_l;
                    }
                    input_sample_l += s.o_ll[s.out_ll as usize];
                    s.d_ll[6] = s.d_ll[5];
                    s.d_ll[5] = s.d_ll[4];
                    s.d_ll[4] = input_sample_l;
                    input_sample_l = (s.d_ll[4] + s.d_ll[5] + s.d_ll[6]) / 3.0;
                    allpasstemp = s.out_ml - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_l -= s.o_ml[allpasstemp as usize] * constallpass;
                    s.o_ml[s.out_ml as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ml -= 1;
                    if s.out_ml < 0 || s.out_ml > s.delay_m {
                        s.out_ml = s.delay_m;
                    }
                    input_sample_l += s.o_ml[s.out_ml as usize];
                    s.d_ml[6] = s.d_ml[5];
                    s.d_ml[5] = s.d_ml[4];
                    s.d_ml[4] = input_sample_l;
                    input_sample_l = (s.d_ml[4] + s.d_ml[5] + s.d_ml[6]) / 3.0;
                    allpasstemp = s.out_nl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_l -= s.o_nl[allpasstemp as usize] * constallpass;
                    s.o_nl[s.out_nl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_nl -= 1;
                    if s.out_nl < 0 || s.out_nl > s.delay_n {
                        s.out_nl = s.delay_n;
                    }
                    input_sample_l += s.o_nl[s.out_nl as usize];
                    s.d_nl[6] = s.d_nl[5];
                    s.d_nl[5] = s.d_nl[4];
                    s.d_nl[4] = input_sample_l;
                    input_sample_l = (s.d_nl[4] + s.d_nl[5] + s.d_nl[6]) / 3.0;
                    allpasstemp = s.out_ol - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_l -= s.o_ol[allpasstemp as usize] * constallpass;
                    s.o_ol[s.out_ol as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ol -= 1;
                    if s.out_ol < 0 || s.out_ol > s.delay_o {
                        s.out_ol = s.delay_o;
                    }
                    input_sample_l += s.o_ol[s.out_ol as usize];
                    s.d_ol[6] = s.d_ol[5];
                    s.d_ol[5] = s.d_ol[4];
                    s.d_ol[4] = input_sample_l;
                    input_sample_l = (s.d_ol[4] + s.d_ol[5] + s.d_ol[6]) / 3.0;
                    allpasstemp = s.out_pl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_l -= s.o_pl[allpasstemp as usize] * constallpass;
                    s.o_pl[s.out_pl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_pl -= 1;
                    if s.out_pl < 0 || s.out_pl > s.delay_p {
                        s.out_pl = s.delay_p;
                    }
                    input_sample_l += s.o_pl[s.out_pl as usize];
                    s.d_pl[6] = s.d_pl[5];
                    s.d_pl[5] = s.d_pl[4];
                    s.d_pl[4] = input_sample_l;
                    input_sample_l = (s.d_pl[4] + s.d_pl[5] + s.d_pl[6]) / 3.0;
                    allpasstemp = s.out_ql - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_l -= s.o_ql[allpasstemp as usize] * constallpass;
                    s.o_ql[s.out_ql as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ql -= 1;
                    if s.out_ql < 0 || s.out_ql > s.delay_q {
                        s.out_ql = s.delay_q;
                    }
                    input_sample_l += s.o_ql[s.out_ql as usize];
                    s.d_ql[6] = s.d_ql[5];
                    s.d_ql[5] = s.d_ql[4];
                    s.d_ql[4] = input_sample_l;
                    input_sample_l = (s.d_ql[4] + s.d_ql[5] + s.d_ql[6]) / 3.0;
                    allpasstemp = s.out_rl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_l -= s.o_rl[allpasstemp as usize] * constallpass;
                    s.o_rl[s.out_rl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_rl -= 1;
                    if s.out_rl < 0 || s.out_rl > s.delay_r {
                        s.out_rl = s.delay_r;
                    }
                    input_sample_l += s.o_rl[s.out_rl as usize];
                    s.d_rl[6] = s.d_rl[5];
                    s.d_rl[5] = s.d_rl[4];
                    s.d_rl[4] = input_sample_l;
                    input_sample_l = (s.d_rl[4] + s.d_rl[5] + s.d_rl[6]) / 3.0;
                    allpasstemp = s.out_sl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_l -= s.o_sl[allpasstemp as usize] * constallpass;
                    s.o_sl[s.out_sl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_sl -= 1;
                    if s.out_sl < 0 || s.out_sl > s.delay_s {
                        s.out_sl = s.delay_s;
                    }
                    input_sample_l += s.o_sl[s.out_sl as usize];
                    s.d_sl[6] = s.d_sl[5];
                    s.d_sl[5] = s.d_sl[4];
                    s.d_sl[4] = input_sample_l;
                    input_sample_l = (s.d_sl[4] + s.d_sl[5] + s.d_sl[6]) / 3.0;
                    allpasstemp = s.out_tl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_l -= s.o_tl[allpasstemp as usize] * constallpass;
                    s.o_tl[s.out_tl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_tl -= 1;
                    if s.out_tl < 0 || s.out_tl > s.delay_t {
                        s.out_tl = s.delay_t;
                    }
                    input_sample_l += s.o_tl[s.out_tl as usize];
                    s.d_tl[6] = s.d_tl[5];
                    s.d_tl[5] = s.d_tl[4];
                    s.d_tl[4] = input_sample_l;
                    input_sample_l = (s.d_tl[4] + s.d_tl[5] + s.d_tl[6]) / 3.0;
                    allpasstemp = s.out_ul - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_l -= s.o_ul[allpasstemp as usize] * constallpass;
                    s.o_ul[s.out_ul as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_ul -= 1;
                    if s.out_ul < 0 || s.out_ul > s.delay_u {
                        s.out_ul = s.delay_u;
                    }
                    input_sample_l += s.o_ul[s.out_ul as usize];
                    s.d_ul[6] = s.d_ul[5];
                    s.d_ul[5] = s.d_ul[4];
                    s.d_ul[4] = input_sample_l;
                    input_sample_l = (s.d_ul[4] + s.d_ul[5] + s.d_ul[6]) / 3.0;
                    allpasstemp = s.out_vl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_l -= s.o_vl[allpasstemp as usize] * constallpass;
                    s.o_vl[s.out_vl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_vl -= 1;
                    if s.out_vl < 0 || s.out_vl > s.delay_v {
                        s.out_vl = s.delay_v;
                    }
                    input_sample_l += s.o_vl[s.out_vl as usize];
                    s.d_vl[6] = s.d_vl[5];
                    s.d_vl[5] = s.d_vl[4];
                    s.d_vl[4] = input_sample_l;
                    input_sample_l = (s.d_vl[4] + s.d_vl[5] + s.d_vl[6]) / 3.0;
                    allpasstemp = s.out_wl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_l -= s.o_wl[allpasstemp as usize] * constallpass;
                    s.o_wl[s.out_wl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_wl -= 1;
                    if s.out_wl < 0 || s.out_wl > s.delay_w {
                        s.out_wl = s.delay_w;
                    }
                    input_sample_l += s.o_wl[s.out_wl as usize];
                    s.d_wl[6] = s.d_wl[5];
                    s.d_wl[5] = s.d_wl[4];
                    s.d_wl[4] = input_sample_l;
                    input_sample_l = (s.d_wl[4] + s.d_wl[5] + s.d_wl[6]) / 3.0;
                    allpasstemp = s.out_xl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_l -= s.o_xl[allpasstemp as usize] * constallpass;
                    s.o_xl[s.out_xl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_xl -= 1;
                    if s.out_xl < 0 || s.out_xl > s.delay_x {
                        s.out_xl = s.delay_x;
                    }
                    input_sample_l += s.o_xl[s.out_xl as usize];
                    s.d_xl[6] = s.d_xl[5];
                    s.d_xl[5] = s.d_xl[4];
                    s.d_xl[4] = input_sample_l;
                    input_sample_l = (s.d_xl[4] + s.d_xl[5] + s.d_xl[6]) / 3.0;
                    allpasstemp = s.out_yl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_l -= s.o_yl[allpasstemp as usize] * constallpass;
                    s.o_yl[s.out_yl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_yl -= 1;
                    if s.out_yl < 0 || s.out_yl > s.delay_y {
                        s.out_yl = s.delay_y;
                    }
                    input_sample_l += s.o_yl[s.out_yl as usize];
                    s.d_yl[6] = s.d_yl[5];
                    s.d_yl[5] = s.d_yl[4];
                    s.d_yl[4] = input_sample_l;
                    input_sample_l = (s.d_yl[4] + s.d_yl[5] + s.d_yl[6]) / 3.0;
                    allpasstemp = s.out_zl - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_l -= s.o_zl[allpasstemp as usize] * constallpass;
                    s.o_zl[s.out_zl as usize] = input_sample_l;
                    input_sample_l *= constallpass;
                    s.out_zl -= 1;
                    if s.out_zl < 0 || s.out_zl > s.delay_z {
                        s.out_zl = s.delay_z;
                    }
                    input_sample_l += s.o_zl[s.out_zl as usize];
                    s.d_zl[6] = s.d_zl[5];
                    s.d_zl[5] = s.d_zl[4];
                    s.d_zl[4] = input_sample_l;
                    input_sample_l = s.d_zl[4] + s.d_zl[5] + s.d_zl[6];
                }
                _ => {}
            }
            match verbtype {
                1 => {
                    allpasstemp = s.alp_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.a_ar[allpasstemp as usize] * constallpass;
                    s.a_ar[s.alp_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ar -= 1;
                    if s.alp_ar < 0 || s.alp_ar > s.delay_a {
                        s.alp_ar = s.delay_a;
                    }
                    input_sample_r += s.a_ar[s.alp_ar as usize];
                    s.d_ar[3] = s.d_ar[2];
                    s.d_ar[2] = s.d_ar[1];
                    s.d_ar[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_ar[2] + s.d_ar[3]) / 3.0;
                    allpasstemp = s.alp_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.a_br[allpasstemp as usize] * constallpass;
                    s.a_br[s.alp_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_br -= 1;
                    if s.alp_br < 0 || s.alp_br > s.delay_b {
                        s.alp_br = s.delay_b;
                    }
                    input_sample_r += s.a_br[s.alp_br as usize];
                    s.d_br[3] = s.d_br[2];
                    s.d_br[2] = s.d_br[1];
                    s.d_br[1] = input_sample_r;
                    input_sample_r = (s.d_br[1] + s.d_br[2] + s.d_br[3]) / 3.0;
                    allpasstemp = s.alp_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.a_cr[allpasstemp as usize] * constallpass;
                    s.a_cr[s.alp_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_cr -= 1;
                    if s.alp_cr < 0 || s.alp_cr > s.delay_c {
                        s.alp_cr = s.delay_c;
                    }
                    input_sample_r += s.a_cr[s.alp_cr as usize];
                    s.d_cr[3] = s.d_cr[2];
                    s.d_cr[2] = s.d_cr[1];
                    s.d_cr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_cr[2] + s.d_cr[3]) / 3.0;
                    allpasstemp = s.alp_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.a_dr[allpasstemp as usize] * constallpass;
                    s.a_dr[s.alp_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_dr -= 1;
                    if s.alp_dr < 0 || s.alp_dr > s.delay_d {
                        s.alp_dr = s.delay_d;
                    }
                    input_sample_r += s.a_dr[s.alp_dr as usize];
                    s.d_dr[3] = s.d_dr[2];
                    s.d_dr[2] = s.d_dr[1];
                    s.d_dr[1] = input_sample_r;
                    input_sample_r = (s.d_dr[1] + s.d_dr[2] + s.d_dr[3]) / 3.0;
                    allpasstemp = s.alp_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.a_er[allpasstemp as usize] * constallpass;
                    s.a_er[s.alp_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_er -= 1;
                    if s.alp_er < 0 || s.alp_er > s.delay_e {
                        s.alp_er = s.delay_e;
                    }
                    input_sample_r += s.a_er[s.alp_er as usize];
                    s.d_er[3] = s.d_er[2];
                    s.d_er[2] = s.d_er[1];
                    s.d_er[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_er[2] + s.d_er[3]) / 3.0;
                    allpasstemp = s.alp_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.a_fr[allpasstemp as usize] * constallpass;
                    s.a_fr[s.alp_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_fr -= 1;
                    if s.alp_fr < 0 || s.alp_fr > s.delay_f {
                        s.alp_fr = s.delay_f;
                    }
                    input_sample_r += s.a_fr[s.alp_fr as usize];
                    s.d_fr[3] = s.d_fr[2];
                    s.d_fr[2] = s.d_fr[1];
                    s.d_fr[1] = input_sample_r;
                    input_sample_r = (s.d_fr[1] + s.d_fr[2] + s.d_fr[3]) / 3.0;
                    allpasstemp = s.alp_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.a_gr[allpasstemp as usize] * constallpass;
                    s.a_gr[s.alp_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_gr -= 1;
                    if s.alp_gr < 0 || s.alp_gr > s.delay_g {
                        s.alp_gr = s.delay_g;
                    }
                    input_sample_r += s.a_gr[s.alp_gr as usize];
                    s.d_gr[3] = s.d_gr[2];
                    s.d_gr[2] = s.d_gr[1];
                    s.d_gr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_gr[2] + s.d_gr[3]) / 3.0;
                    allpasstemp = s.alp_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.a_hr[allpasstemp as usize] * constallpass;
                    s.a_hr[s.alp_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_hr -= 1;
                    if s.alp_hr < 0 || s.alp_hr > s.delay_h {
                        s.alp_hr = s.delay_h;
                    }
                    input_sample_r += s.a_hr[s.alp_hr as usize];
                    s.d_hr[3] = s.d_hr[2];
                    s.d_hr[2] = s.d_hr[1];
                    s.d_hr[1] = input_sample_r;
                    input_sample_r = (s.d_hr[1] + s.d_hr[2] + s.d_hr[3]) / 3.0;
                    allpasstemp = s.alp_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.a_ir[allpasstemp as usize] * constallpass;
                    s.a_ir[s.alp_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ir -= 1;
                    if s.alp_ir < 0 || s.alp_ir > s.delay_i {
                        s.alp_ir = s.delay_i;
                    }
                    input_sample_r += s.a_ir[s.alp_ir as usize];
                    s.d_ir[3] = s.d_ir[2];
                    s.d_ir[2] = s.d_ir[1];
                    s.d_ir[1] = input_sample_r;
                    input_sample_r = (s.d_ir[1] + s.d_ir[2] + s.d_ir[3]) / 3.0;
                    allpasstemp = s.alp_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.a_jr[allpasstemp as usize] * constallpass;
                    s.a_jr[s.alp_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_jr -= 1;
                    if s.alp_jr < 0 || s.alp_jr > s.delay_j {
                        s.alp_jr = s.delay_j;
                    }
                    input_sample_r += s.a_jr[s.alp_jr as usize];
                    s.d_jr[3] = s.d_jr[2];
                    s.d_jr[2] = s.d_jr[1];
                    s.d_jr[1] = input_sample_r;
                    input_sample_r = (s.d_jr[1] + s.d_jr[2] + s.d_jr[3]) / 3.0;
                    allpasstemp = s.alp_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.a_kr[allpasstemp as usize] * constallpass;
                    s.a_kr[s.alp_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_kr -= 1;
                    if s.alp_kr < 0 || s.alp_kr > s.delay_k {
                        s.alp_kr = s.delay_k;
                    }
                    input_sample_r += s.a_kr[s.alp_kr as usize];
                    s.d_kr[3] = s.d_kr[2];
                    s.d_kr[2] = s.d_kr[1];
                    s.d_kr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_kr[2] + s.d_kr[3]) / 3.0;
                    allpasstemp = s.alp_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.a_lr[allpasstemp as usize] * constallpass;
                    s.a_lr[s.alp_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_lr -= 1;
                    if s.alp_lr < 0 || s.alp_lr > s.delay_l {
                        s.alp_lr = s.delay_l;
                    }
                    input_sample_r += s.a_lr[s.alp_lr as usize];
                    s.d_lr[3] = s.d_lr[2];
                    s.d_lr[2] = s.d_lr[1];
                    s.d_lr[1] = input_sample_r;
                    input_sample_r = (s.d_lr[1] + s.d_lr[2] + s.d_lr[3]) / 3.0;
                    allpasstemp = s.alp_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.a_mr[allpasstemp as usize] * constallpass;
                    s.a_mr[s.alp_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_mr -= 1;
                    if s.alp_mr < 0 || s.alp_mr > s.delay_m {
                        s.alp_mr = s.delay_m;
                    }
                    input_sample_r += s.a_mr[s.alp_mr as usize];
                    s.d_mr[3] = s.d_mr[2];
                    s.d_mr[2] = s.d_mr[1];
                    s.d_mr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_mr[2] + s.d_mr[3]) / 3.0;
                    allpasstemp = s.alp_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.a_nr[allpasstemp as usize] * constallpass;
                    s.a_nr[s.alp_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_nr -= 1;
                    if s.alp_nr < 0 || s.alp_nr > s.delay_n {
                        s.alp_nr = s.delay_n;
                    }
                    input_sample_r += s.a_nr[s.alp_nr as usize];
                    s.d_nr[3] = s.d_nr[2];
                    s.d_nr[2] = s.d_nr[1];
                    s.d_nr[1] = input_sample_r;
                    input_sample_r = (s.d_nr[1] + s.d_nr[2] + s.d_nr[3]) / 3.0;
                    allpasstemp = s.alp_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.a_or[allpasstemp as usize] * constallpass;
                    s.a_or[s.alp_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_or -= 1;
                    if s.alp_or < 0 || s.alp_or > s.delay_o {
                        s.alp_or = s.delay_o;
                    }
                    input_sample_r += s.a_or[s.alp_or as usize];
                    s.d_or[3] = s.d_or[2];
                    s.d_or[2] = s.d_or[1];
                    s.d_or[1] = input_sample_r;
                    input_sample_r = (s.d_or[1] + s.d_or[2] + s.d_or[3]) / 3.0;
                    allpasstemp = s.alp_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.a_pr[allpasstemp as usize] * constallpass;
                    s.a_pr[s.alp_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_pr -= 1;
                    if s.alp_pr < 0 || s.alp_pr > s.delay_p {
                        s.alp_pr = s.delay_p;
                    }
                    input_sample_r += s.a_pr[s.alp_pr as usize];
                    s.d_pr[3] = s.d_pr[2];
                    s.d_pr[2] = s.d_pr[1];
                    s.d_pr[1] = input_sample_r;
                    input_sample_r = (s.d_pr[1] + s.d_pr[2] + s.d_pr[3]) / 3.0;
                    allpasstemp = s.alp_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.a_qr[allpasstemp as usize] * constallpass;
                    s.a_qr[s.alp_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_qr -= 1;
                    if s.alp_qr < 0 || s.alp_qr > s.delay_q {
                        s.alp_qr = s.delay_q;
                    }
                    input_sample_r += s.a_qr[s.alp_qr as usize];
                    s.d_qr[3] = s.d_qr[2];
                    s.d_qr[2] = s.d_qr[1];
                    s.d_qr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_qr[2] + s.d_qr[3]) / 3.0;
                    allpasstemp = s.alp_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.a_rr[allpasstemp as usize] * constallpass;
                    s.a_rr[s.alp_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_rr -= 1;
                    if s.alp_rr < 0 || s.alp_rr > s.delay_r {
                        s.alp_rr = s.delay_r;
                    }
                    input_sample_r += s.a_rr[s.alp_rr as usize];
                    s.d_rr[3] = s.d_rr[2];
                    s.d_rr[2] = s.d_rr[1];
                    s.d_rr[1] = input_sample_r;
                    input_sample_r = (s.d_rr[1] + s.d_rr[2] + s.d_rr[3]) / 3.0;
                    allpasstemp = s.alp_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.a_sr[allpasstemp as usize] * constallpass;
                    s.a_sr[s.alp_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_sr -= 1;
                    if s.alp_sr < 0 || s.alp_sr > s.delay_s {
                        s.alp_sr = s.delay_s;
                    }
                    input_sample_r += s.a_sr[s.alp_sr as usize];
                    s.d_sr[3] = s.d_sr[2];
                    s.d_sr[2] = s.d_sr[1];
                    s.d_sr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_sr[2] + s.d_sr[3]) / 3.0;
                    allpasstemp = s.alp_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.a_tr[allpasstemp as usize] * constallpass;
                    s.a_tr[s.alp_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_tr -= 1;
                    if s.alp_tr < 0 || s.alp_tr > s.delay_t {
                        s.alp_tr = s.delay_t;
                    }
                    input_sample_r += s.a_tr[s.alp_tr as usize];
                    s.d_tr[3] = s.d_tr[2];
                    s.d_tr[2] = s.d_tr[1];
                    s.d_tr[1] = input_sample_r;
                    input_sample_r = (s.d_tr[1] + s.d_tr[2] + s.d_tr[3]) / 3.0;
                    allpasstemp = s.alp_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.a_ur[allpasstemp as usize] * constallpass;
                    s.a_ur[s.alp_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ur -= 1;
                    if s.alp_ur < 0 || s.alp_ur > s.delay_u {
                        s.alp_ur = s.delay_u;
                    }
                    input_sample_r += s.a_ur[s.alp_ur as usize];
                    s.d_ur[3] = s.d_ur[2];
                    s.d_ur[2] = s.d_ur[1];
                    s.d_ur[1] = input_sample_r;
                    input_sample_r = (s.d_ur[1] + s.d_ur[2] + s.d_ur[3]) / 3.0;
                    allpasstemp = s.alp_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.a_vr[allpasstemp as usize] * constallpass;
                    s.a_vr[s.alp_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_vr -= 1;
                    if s.alp_vr < 0 || s.alp_vr > s.delay_v {
                        s.alp_vr = s.delay_v;
                    }
                    input_sample_r += s.a_vr[s.alp_vr as usize];
                    s.d_vr[3] = s.d_vr[2];
                    s.d_vr[2] = s.d_vr[1];
                    s.d_vr[1] = input_sample_r;
                    input_sample_r = (s.d_vr[1] + s.d_vr[2] + s.d_vr[3]) / 3.0;
                    allpasstemp = s.alp_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.a_wr[allpasstemp as usize] * constallpass;
                    s.a_wr[s.alp_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_wr -= 1;
                    if s.alp_wr < 0 || s.alp_wr > s.delay_w {
                        s.alp_wr = s.delay_w;
                    }
                    input_sample_r += s.a_wr[s.alp_wr as usize];
                    s.d_wr[3] = s.d_wr[2];
                    s.d_wr[2] = s.d_wr[1];
                    s.d_wr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_wr[2] + s.d_wr[3]) / 3.0;
                    allpasstemp = s.alp_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.a_xr[allpasstemp as usize] * constallpass;
                    s.a_xr[s.alp_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_xr -= 1;
                    if s.alp_xr < 0 || s.alp_xr > s.delay_x {
                        s.alp_xr = s.delay_x;
                    }
                    input_sample_r += s.a_xr[s.alp_xr as usize];
                    s.d_xr[3] = s.d_xr[2];
                    s.d_xr[2] = s.d_xr[1];
                    s.d_xr[1] = input_sample_r;
                    input_sample_r = (s.d_xr[1] + s.d_xr[2] + s.d_xr[3]) / 3.0;
                    allpasstemp = s.alp_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.a_yr[allpasstemp as usize] * constallpass;
                    s.a_yr[s.alp_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_yr -= 1;
                    if s.alp_yr < 0 || s.alp_yr > s.delay_y {
                        s.alp_yr = s.delay_y;
                    }
                    input_sample_r += s.a_yr[s.alp_yr as usize];
                    s.d_yr[3] = s.d_yr[2];
                    s.d_yr[2] = s.d_yr[1];
                    s.d_yr[1] = input_sample_r;
                    input_sample_r = (s.d_yr[1] + s.d_yr[2] + s.d_yr[3]) / 3.0;
                    allpasstemp = s.alp_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.a_zr[allpasstemp as usize] * constallpass;
                    s.a_zr[s.alp_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_zr -= 1;
                    if s.alp_zr < 0 || s.alp_zr > s.delay_z {
                        s.alp_zr = s.delay_z;
                    }
                    input_sample_r += s.a_zr[s.alp_zr as usize];
                    s.d_zr[3] = s.d_zr[2];
                    s.d_zr[2] = s.d_zr[1];
                    s.d_zr[1] = input_sample_r;
                    input_sample_r = (s.d_zr[1] + s.d_zr[2] + s.d_zr[3]) / 3.0;
                    allpasstemp = s.out_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.o_ar[allpasstemp as usize] * constallpass;
                    s.o_ar[s.out_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ar -= 1;
                    if s.out_ar < 0 || s.out_ar > s.delay_a {
                        s.out_ar = s.delay_a;
                    }
                    input_sample_r += s.o_ar[s.out_ar as usize];
                    s.d_ar[6] = s.d_ar[5];
                    s.d_ar[5] = s.d_ar[4];
                    s.d_ar[4] = input_sample_r;
                    input_sample_r = (s.d_ar[4] + s.d_ar[5] + s.d_ar[6]) / 3.0;
                    allpasstemp = s.out_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.o_br[allpasstemp as usize] * constallpass;
                    s.o_br[s.out_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_br -= 1;
                    if s.out_br < 0 || s.out_br > s.delay_b {
                        s.out_br = s.delay_b;
                    }
                    input_sample_r += s.o_br[s.out_br as usize];
                    s.d_br[6] = s.d_br[5];
                    s.d_br[5] = s.d_br[4];
                    s.d_br[4] = input_sample_r;
                    input_sample_r = (s.d_br[4] + s.d_br[5] + s.d_br[6]) / 3.0;
                    allpasstemp = s.out_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.o_cr[allpasstemp as usize] * constallpass;
                    s.o_cr[s.out_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_cr -= 1;
                    if s.out_cr < 0 || s.out_cr > s.delay_c {
                        s.out_cr = s.delay_c;
                    }
                    input_sample_r += s.o_cr[s.out_cr as usize];
                    s.d_cr[6] = s.d_cr[5];
                    s.d_cr[5] = s.d_cr[4];
                    s.d_cr[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_cr[5] + s.d_cr[6]) / 3.0;
                    allpasstemp = s.out_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.o_dr[allpasstemp as usize] * constallpass;
                    s.o_dr[s.out_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_dr -= 1;
                    if s.out_dr < 0 || s.out_dr > s.delay_d {
                        s.out_dr = s.delay_d;
                    }
                    input_sample_r += s.o_dr[s.out_dr as usize];
                    s.d_dr[6] = s.d_dr[5];
                    s.d_dr[5] = s.d_dr[4];
                    s.d_dr[4] = input_sample_r;
                    input_sample_r = (s.d_dr[4] + s.d_dr[5] + s.d_dr[6]) / 3.0;
                    allpasstemp = s.out_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.o_er[allpasstemp as usize] * constallpass;
                    s.o_er[s.out_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_er -= 1;
                    if s.out_er < 0 || s.out_er > s.delay_e {
                        s.out_er = s.delay_e;
                    }
                    input_sample_r += s.o_er[s.out_er as usize];
                    s.d_er[6] = s.d_er[5];
                    s.d_er[5] = s.d_er[4];
                    s.d_er[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_er[5] + s.d_er[6]) / 3.0;
                    allpasstemp = s.out_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.o_fr[allpasstemp as usize] * constallpass;
                    s.o_fr[s.out_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_fr -= 1;
                    if s.out_fr < 0 || s.out_fr > s.delay_f {
                        s.out_fr = s.delay_f;
                    }
                    input_sample_r += s.o_fr[s.out_fr as usize];
                    s.d_fr[6] = s.d_fr[5];
                    s.d_fr[5] = s.d_fr[4];
                    s.d_fr[4] = input_sample_r;
                    input_sample_r = (s.d_fr[4] + s.d_fr[5] + s.d_fr[6]) / 3.0;
                    allpasstemp = s.out_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.o_gr[allpasstemp as usize] * constallpass;
                    s.o_gr[s.out_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_gr -= 1;
                    if s.out_gr < 0 || s.out_gr > s.delay_g {
                        s.out_gr = s.delay_g;
                    }
                    input_sample_r += s.o_gr[s.out_gr as usize];
                    s.d_gr[6] = s.d_gr[5];
                    s.d_gr[5] = s.d_gr[4];
                    s.d_gr[4] = input_sample_r;
                    input_sample_r = (s.d_gr[4] + s.d_gr[5] + s.d_gr[6]) / 3.0;
                    allpasstemp = s.out_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.o_hr[allpasstemp as usize] * constallpass;
                    s.o_hr[s.out_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_hr -= 1;
                    if s.out_hr < 0 || s.out_hr > s.delay_h {
                        s.out_hr = s.delay_h;
                    }
                    input_sample_r += s.o_hr[s.out_hr as usize];
                    s.d_hr[6] = s.d_hr[5];
                    s.d_hr[5] = s.d_hr[4];
                    s.d_hr[4] = input_sample_r;
                    input_sample_r = (s.d_hr[4] + s.d_hr[5] + s.d_hr[6]) / 3.0;
                    allpasstemp = s.out_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.o_ir[allpasstemp as usize] * constallpass;
                    s.o_ir[s.out_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ir -= 1;
                    if s.out_ir < 0 || s.out_ir > s.delay_i {
                        s.out_ir = s.delay_i;
                    }
                    input_sample_r += s.o_ir[s.out_ir as usize];
                    s.d_ir[6] = s.d_ir[5];
                    s.d_ir[5] = s.d_ir[4];
                    s.d_ir[4] = input_sample_r;
                    input_sample_r = (s.d_ir[4] + s.d_ir[5] + s.d_ir[6]) / 3.0;
                    allpasstemp = s.out_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.o_jr[allpasstemp as usize] * constallpass;
                    s.o_jr[s.out_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_jr -= 1;
                    if s.out_jr < 0 || s.out_jr > s.delay_j {
                        s.out_jr = s.delay_j;
                    }
                    input_sample_r += s.o_jr[s.out_jr as usize];
                    s.d_jr[6] = s.d_jr[5];
                    s.d_jr[5] = s.d_jr[4];
                    s.d_jr[4] = input_sample_r;
                    input_sample_r = (s.d_jr[4] + s.d_jr[5] + s.d_jr[6]) / 3.0;
                    allpasstemp = s.out_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.o_kr[allpasstemp as usize] * constallpass;
                    s.o_kr[s.out_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_kr -= 1;
                    if s.out_kr < 0 || s.out_kr > s.delay_k {
                        s.out_kr = s.delay_k;
                    }
                    input_sample_r += s.o_kr[s.out_kr as usize];
                    s.d_kr[6] = s.d_kr[5];
                    s.d_kr[5] = s.d_kr[4];
                    s.d_kr[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_kr[5] + s.d_kr[6]) / 3.0;
                    allpasstemp = s.out_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.o_lr[allpasstemp as usize] * constallpass;
                    s.o_lr[s.out_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_lr -= 1;
                    if s.out_lr < 0 || s.out_lr > s.delay_l {
                        s.out_lr = s.delay_l;
                    }
                    input_sample_r += s.o_lr[s.out_lr as usize];
                    s.d_lr[6] = s.d_lr[5];
                    s.d_lr[5] = s.d_lr[4];
                    s.d_lr[4] = input_sample_r;
                    input_sample_r = (s.d_lr[4] + s.d_lr[5] + s.d_lr[6]) / 3.0;
                    allpasstemp = s.out_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.o_mr[allpasstemp as usize] * constallpass;
                    s.o_mr[s.out_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_mr -= 1;
                    if s.out_mr < 0 || s.out_mr > s.delay_m {
                        s.out_mr = s.delay_m;
                    }
                    input_sample_r += s.o_mr[s.out_mr as usize];
                    s.d_mr[6] = s.d_mr[5];
                    s.d_mr[5] = s.d_mr[4];
                    s.d_mr[4] = input_sample_r;
                    input_sample_r = (s.d_mr[4] + s.d_mr[5] + s.d_mr[6]) / 3.0;
                    allpasstemp = s.out_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.o_nr[allpasstemp as usize] * constallpass;
                    s.o_nr[s.out_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_nr -= 1;
                    if s.out_nr < 0 || s.out_nr > s.delay_n {
                        s.out_nr = s.delay_n;
                    }
                    input_sample_r += s.o_nr[s.out_nr as usize];
                    s.d_nr[6] = s.d_nr[5];
                    s.d_nr[5] = s.d_nr[4];
                    s.d_nr[4] = input_sample_r;
                    input_sample_r = (s.d_nr[4] + s.d_nr[5] + s.d_nr[6]) / 3.0;
                    allpasstemp = s.out_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.o_or[allpasstemp as usize] * constallpass;
                    s.o_or[s.out_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_or -= 1;
                    if s.out_or < 0 || s.out_or > s.delay_o {
                        s.out_or = s.delay_o;
                    }
                    input_sample_r += s.o_or[s.out_or as usize];
                    s.d_or[6] = s.d_or[5];
                    s.d_or[5] = s.d_or[4];
                    s.d_or[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_or[5] + s.d_or[6]) / 3.0;
                    allpasstemp = s.out_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.o_pr[allpasstemp as usize] * constallpass;
                    s.o_pr[s.out_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_pr -= 1;
                    if s.out_pr < 0 || s.out_pr > s.delay_p {
                        s.out_pr = s.delay_p;
                    }
                    input_sample_r += s.o_pr[s.out_pr as usize];
                    s.d_pr[6] = s.d_pr[5];
                    s.d_pr[5] = s.d_pr[4];
                    s.d_pr[4] = input_sample_r;
                    input_sample_r = (s.d_pr[4] + s.d_pr[5] + s.d_pr[6]) / 3.0;
                    allpasstemp = s.out_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.o_qr[allpasstemp as usize] * constallpass;
                    s.o_qr[s.out_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_qr -= 1;
                    if s.out_qr < 0 || s.out_qr > s.delay_q {
                        s.out_qr = s.delay_q;
                    }
                    input_sample_r += s.o_qr[s.out_qr as usize];
                    s.d_qr[6] = s.d_qr[5];
                    s.d_qr[5] = s.d_qr[4];
                    s.d_qr[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_qr[5] + s.d_qr[6]) / 3.0;
                    allpasstemp = s.out_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.o_rr[allpasstemp as usize] * constallpass;
                    s.o_rr[s.out_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_rr -= 1;
                    if s.out_rr < 0 || s.out_rr > s.delay_r {
                        s.out_rr = s.delay_r;
                    }
                    input_sample_r += s.o_rr[s.out_rr as usize];
                    s.d_rr[6] = s.d_rr[5];
                    s.d_rr[5] = s.d_rr[4];
                    s.d_rr[4] = input_sample_r;
                    input_sample_r = (s.d_rr[4] + s.d_rr[5] + s.d_rr[6]) / 3.0;
                    allpasstemp = s.out_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.o_sr[allpasstemp as usize] * constallpass;
                    s.o_sr[s.out_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_sr -= 1;
                    if s.out_sr < 0 || s.out_sr > s.delay_s {
                        s.out_sr = s.delay_s;
                    }
                    input_sample_r += s.o_sr[s.out_sr as usize];
                    s.d_sr[6] = s.d_sr[5];
                    s.d_sr[5] = s.d_sr[4];
                    s.d_sr[4] = input_sample_r;
                    input_sample_r = (s.d_sr[4] + s.d_sr[5] + s.d_sr[6]) / 3.0;
                    allpasstemp = s.out_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.o_tr[allpasstemp as usize] * constallpass;
                    s.o_tr[s.out_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_tr -= 1;
                    if s.out_tr < 0 || s.out_tr > s.delay_t {
                        s.out_tr = s.delay_t;
                    }
                    input_sample_r += s.o_tr[s.out_tr as usize];
                    s.d_tr[6] = s.d_tr[5];
                    s.d_tr[5] = s.d_tr[4];
                    s.d_tr[4] = input_sample_r;
                    input_sample_r = (s.d_tr[4] + s.d_tr[5] + s.d_tr[6]) / 3.0;
                    allpasstemp = s.out_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.o_ur[allpasstemp as usize] * constallpass;
                    s.o_ur[s.out_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ur -= 1;
                    if s.out_ur < 0 || s.out_ur > s.delay_u {
                        s.out_ur = s.delay_u;
                    }
                    input_sample_r += s.o_ur[s.out_ur as usize];
                    s.d_ur[6] = s.d_ur[5];
                    s.d_ur[5] = s.d_ur[4];
                    s.d_ur[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_ur[5] + s.d_ur[6]) / 3.0;
                    allpasstemp = s.out_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.o_vr[allpasstemp as usize] * constallpass;
                    s.o_vr[s.out_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_vr -= 1;
                    if s.out_vr < 0 || s.out_vr > s.delay_v {
                        s.out_vr = s.delay_v;
                    }
                    input_sample_r += s.o_vr[s.out_vr as usize];
                    s.d_vr[6] = s.d_vr[5];
                    s.d_vr[5] = s.d_vr[4];
                    s.d_vr[4] = input_sample_r;
                    input_sample_r = (s.d_vr[4] + s.d_vr[5] + s.d_vr[6]) / 3.0;
                    allpasstemp = s.out_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.o_wr[allpasstemp as usize] * constallpass;
                    s.o_wr[s.out_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_wr -= 1;
                    if s.out_wr < 0 || s.out_wr > s.delay_w {
                        s.out_wr = s.delay_w;
                    }
                    input_sample_r += s.o_wr[s.out_wr as usize];
                    s.d_wr[6] = s.d_wr[5];
                    s.d_wr[5] = s.d_wr[4];
                    s.d_wr[4] = input_sample_r;
                    input_sample_r = (s.d_wr[4] + s.d_wr[5] + s.d_wr[6]) / 3.0;
                    allpasstemp = s.out_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.o_xr[allpasstemp as usize] * constallpass;
                    s.o_xr[s.out_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_xr -= 1;
                    if s.out_xr < 0 || s.out_xr > s.delay_x {
                        s.out_xr = s.delay_x;
                    }
                    input_sample_r += s.o_xr[s.out_xr as usize];
                    s.d_xr[6] = s.d_xr[5];
                    s.d_xr[5] = s.d_xr[4];
                    s.d_xr[4] = input_sample_r;
                    input_sample_r = (s.d_xr[4] + s.d_xr[5] + s.d_xr[6]) / 3.0;
                    allpasstemp = s.out_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.o_yr[allpasstemp as usize] * constallpass;
                    s.o_yr[s.out_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_yr -= 1;
                    if s.out_yr < 0 || s.out_yr > s.delay_y {
                        s.out_yr = s.delay_y;
                    }
                    input_sample_r += s.o_yr[s.out_yr as usize];
                    s.d_yr[6] = s.d_yr[5];
                    s.d_yr[5] = s.d_yr[4];
                    s.d_yr[4] = input_sample_r;
                    input_sample_r = (s.d_yr[4] + s.d_yr[5] + s.d_yr[6]) / 3.0;
                    allpasstemp = s.out_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.o_zr[allpasstemp as usize] * constallpass;
                    s.o_zr[s.out_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_zr -= 1;
                    if s.out_zr < 0 || s.out_zr > s.delay_z {
                        s.out_zr = s.delay_z;
                    }
                    input_sample_r += s.o_zr[s.out_zr as usize];
                    s.d_zr[6] = s.d_zr[5];
                    s.d_zr[5] = s.d_zr[4];
                    s.d_zr[4] = input_sample_r;
                    input_sample_r = s.d_zr[4] + s.d_zr[5] + s.d_zr[6];
                }
                2 => {
                    allpasstemp = s.alp_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.a_ar[allpasstemp as usize] * constallpass;
                    s.a_ar[s.alp_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ar -= 1;
                    if s.alp_ar < 0 || s.alp_ar > s.delay_a {
                        s.alp_ar = s.delay_a;
                    }
                    input_sample_r += s.a_ar[s.alp_ar as usize];
                    s.d_ar[3] = s.d_ar[2];
                    s.d_ar[2] = s.d_ar[1];
                    s.d_ar[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_ar[2] + s.d_ar[3]) / 3.0;
                    allpasstemp = s.alp_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.a_br[allpasstemp as usize] * constallpass;
                    s.a_br[s.alp_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_br -= 1;
                    if s.alp_br < 0 || s.alp_br > s.delay_b {
                        s.alp_br = s.delay_b;
                    }
                    input_sample_r += s.a_br[s.alp_br as usize];
                    s.d_br[3] = s.d_br[2];
                    s.d_br[2] = s.d_br[1];
                    s.d_br[1] = input_sample_r;
                    input_sample_r = (s.d_br[1] + s.d_br[2] + s.d_br[3]) / 3.0;
                    allpasstemp = s.alp_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.a_cr[allpasstemp as usize] * constallpass;
                    s.a_cr[s.alp_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_cr -= 1;
                    if s.alp_cr < 0 || s.alp_cr > s.delay_c {
                        s.alp_cr = s.delay_c;
                    }
                    input_sample_r += s.a_cr[s.alp_cr as usize];
                    s.d_cr[3] = s.d_cr[2];
                    s.d_cr[2] = s.d_cr[1];
                    s.d_cr[1] = input_sample_r;
                    input_sample_r = (s.d_cr[1] + s.d_cr[2] + s.d_cr[3]) / 3.0;
                    allpasstemp = s.alp_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.a_dr[allpasstemp as usize] * constallpass;
                    s.a_dr[s.alp_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_dr -= 1;
                    if s.alp_dr < 0 || s.alp_dr > s.delay_d {
                        s.alp_dr = s.delay_d;
                    }
                    input_sample_r += s.a_dr[s.alp_dr as usize];
                    s.d_dr[3] = s.d_dr[2];
                    s.d_dr[2] = s.d_dr[1];
                    s.d_dr[1] = input_sample_r;
                    input_sample_r = (s.d_dr[1] + s.d_dr[2] + s.d_dr[3]) / 3.0;
                    allpasstemp = s.alp_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.a_er[allpasstemp as usize] * constallpass;
                    s.a_er[s.alp_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_er -= 1;
                    if s.alp_er < 0 || s.alp_er > s.delay_e {
                        s.alp_er = s.delay_e;
                    }
                    input_sample_r += s.a_er[s.alp_er as usize];
                    s.d_er[3] = s.d_er[2];
                    s.d_er[2] = s.d_er[1];
                    s.d_er[1] = input_sample_r;
                    input_sample_r = (s.d_er[1] + s.d_er[2] + s.d_er[3]) / 3.0;
                    allpasstemp = s.alp_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.a_fr[allpasstemp as usize] * constallpass;
                    s.a_fr[s.alp_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_fr -= 1;
                    if s.alp_fr < 0 || s.alp_fr > s.delay_f {
                        s.alp_fr = s.delay_f;
                    }
                    input_sample_r += s.a_fr[s.alp_fr as usize];
                    s.d_fr[3] = s.d_fr[2];
                    s.d_fr[2] = s.d_fr[1];
                    s.d_fr[1] = input_sample_r;
                    input_sample_r = (s.d_fr[1] + s.d_fr[2] + s.d_fr[3]) / 3.0;
                    allpasstemp = s.alp_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.a_gr[allpasstemp as usize] * constallpass;
                    s.a_gr[s.alp_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_gr -= 1;
                    if s.alp_gr < 0 || s.alp_gr > s.delay_g {
                        s.alp_gr = s.delay_g;
                    }
                    input_sample_r += s.a_gr[s.alp_gr as usize];
                    s.d_gr[3] = s.d_gr[2];
                    s.d_gr[2] = s.d_gr[1];
                    s.d_gr[1] = input_sample_r;
                    input_sample_r = (s.d_gr[1] + s.d_gr[2] + s.d_gr[3]) / 3.0;
                    allpasstemp = s.alp_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.a_hr[allpasstemp as usize] * constallpass;
                    s.a_hr[s.alp_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_hr -= 1;
                    if s.alp_hr < 0 || s.alp_hr > s.delay_h {
                        s.alp_hr = s.delay_h;
                    }
                    input_sample_r += s.a_hr[s.alp_hr as usize];
                    s.d_hr[3] = s.d_hr[2];
                    s.d_hr[2] = s.d_hr[1];
                    s.d_hr[1] = input_sample_r;
                    input_sample_r = (s.d_hr[1] + s.d_hr[2] + s.d_hr[3]) / 3.0;
                    allpasstemp = s.alp_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.a_ir[allpasstemp as usize] * constallpass;
                    s.a_ir[s.alp_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ir -= 1;
                    if s.alp_ir < 0 || s.alp_ir > s.delay_i {
                        s.alp_ir = s.delay_i;
                    }
                    input_sample_r += s.a_ir[s.alp_ir as usize];
                    s.d_ir[3] = s.d_ir[2];
                    s.d_ir[2] = s.d_ir[1];
                    s.d_ir[1] = input_sample_r;
                    input_sample_r = (s.d_ir[1] + s.d_ir[2] + s.d_ir[3]) / 3.0;
                    allpasstemp = s.alp_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.a_jr[allpasstemp as usize] * constallpass;
                    s.a_jr[s.alp_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_jr -= 1;
                    if s.alp_jr < 0 || s.alp_jr > s.delay_j {
                        s.alp_jr = s.delay_j;
                    }
                    input_sample_r += s.a_jr[s.alp_jr as usize];
                    s.d_jr[3] = s.d_jr[2];
                    s.d_jr[2] = s.d_jr[1];
                    s.d_jr[1] = input_sample_r;
                    input_sample_r = (s.d_jr[1] + s.d_jr[2] + s.d_jr[3]) / 3.0;
                    allpasstemp = s.alp_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.a_kr[allpasstemp as usize] * constallpass;
                    s.a_kr[s.alp_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_kr -= 1;
                    if s.alp_kr < 0 || s.alp_kr > s.delay_k {
                        s.alp_kr = s.delay_k;
                    }
                    input_sample_r += s.a_kr[s.alp_kr as usize];
                    s.d_kr[3] = s.d_kr[2];
                    s.d_kr[2] = s.d_kr[1];
                    s.d_kr[1] = input_sample_r;
                    input_sample_r = (s.d_kr[1] + s.d_kr[2] + s.d_kr[3]) / 3.0;
                    allpasstemp = s.alp_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.a_lr[allpasstemp as usize] * constallpass;
                    s.a_lr[s.alp_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_lr -= 1;
                    if s.alp_lr < 0 || s.alp_lr > s.delay_l {
                        s.alp_lr = s.delay_l;
                    }
                    input_sample_r += s.a_lr[s.alp_lr as usize];
                    s.d_lr[3] = s.d_lr[2];
                    s.d_lr[2] = s.d_lr[1];
                    s.d_lr[1] = input_sample_r;
                    input_sample_r = (s.d_lr[1] + s.d_lr[2] + s.d_lr[3]) / 3.0;
                    allpasstemp = s.alp_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.a_mr[allpasstemp as usize] * constallpass;
                    s.a_mr[s.alp_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_mr -= 1;
                    if s.alp_mr < 0 || s.alp_mr > s.delay_m {
                        s.alp_mr = s.delay_m;
                    }
                    input_sample_r += s.a_mr[s.alp_mr as usize];
                    s.d_mr[3] = s.d_mr[2];
                    s.d_mr[2] = s.d_mr[1];
                    s.d_mr[1] = input_sample_r;
                    input_sample_r = (s.d_mr[1] + s.d_mr[2] + s.d_mr[3]) / 3.0;
                    allpasstemp = s.alp_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.a_nr[allpasstemp as usize] * constallpass;
                    s.a_nr[s.alp_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_nr -= 1;
                    if s.alp_nr < 0 || s.alp_nr > s.delay_n {
                        s.alp_nr = s.delay_n;
                    }
                    input_sample_r += s.a_nr[s.alp_nr as usize];
                    s.d_nr[3] = s.d_nr[2];
                    s.d_nr[2] = s.d_nr[1];
                    s.d_nr[1] = input_sample_r;
                    input_sample_r = (s.d_nr[1] + s.d_nr[2] + s.d_nr[3]) / 3.0;
                    allpasstemp = s.alp_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.a_or[allpasstemp as usize] * constallpass;
                    s.a_or[s.alp_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_or -= 1;
                    if s.alp_or < 0 || s.alp_or > s.delay_o {
                        s.alp_or = s.delay_o;
                    }
                    input_sample_r += s.a_or[s.alp_or as usize];
                    s.d_or[3] = s.d_or[2];
                    s.d_or[2] = s.d_or[1];
                    s.d_or[1] = input_sample_r;
                    input_sample_r = (s.d_or[1] + s.d_or[2] + s.d_or[3]) / 3.0;
                    allpasstemp = s.alp_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.a_pr[allpasstemp as usize] * constallpass;
                    s.a_pr[s.alp_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_pr -= 1;
                    if s.alp_pr < 0 || s.alp_pr > s.delay_p {
                        s.alp_pr = s.delay_p;
                    }
                    input_sample_r += s.a_pr[s.alp_pr as usize];
                    s.d_pr[3] = s.d_pr[2];
                    s.d_pr[2] = s.d_pr[1];
                    s.d_pr[1] = input_sample_r;
                    input_sample_r = (s.d_pr[1] + s.d_pr[2] + s.d_pr[3]) / 3.0;
                    allpasstemp = s.alp_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.a_qr[allpasstemp as usize] * constallpass;
                    s.a_qr[s.alp_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_qr -= 1;
                    if s.alp_qr < 0 || s.alp_qr > s.delay_q {
                        s.alp_qr = s.delay_q;
                    }
                    input_sample_r += s.a_qr[s.alp_qr as usize];
                    s.d_qr[3] = s.d_qr[2];
                    s.d_qr[2] = s.d_qr[1];
                    s.d_qr[1] = input_sample_r;
                    input_sample_r = (s.d_qr[1] + s.d_qr[2] + s.d_qr[3]) / 3.0;
                    allpasstemp = s.alp_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.a_rr[allpasstemp as usize] * constallpass;
                    s.a_rr[s.alp_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_rr -= 1;
                    if s.alp_rr < 0 || s.alp_rr > s.delay_r {
                        s.alp_rr = s.delay_r;
                    }
                    input_sample_r += s.a_rr[s.alp_rr as usize];
                    s.d_rr[3] = s.d_rr[2];
                    s.d_rr[2] = s.d_rr[1];
                    s.d_rr[1] = input_sample_r;
                    input_sample_r = (s.d_rr[1] + s.d_rr[2] + s.d_rr[3]) / 3.0;
                    allpasstemp = s.alp_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.a_sr[allpasstemp as usize] * constallpass;
                    s.a_sr[s.alp_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_sr -= 1;
                    if s.alp_sr < 0 || s.alp_sr > s.delay_s {
                        s.alp_sr = s.delay_s;
                    }
                    input_sample_r += s.a_sr[s.alp_sr as usize];
                    s.d_sr[3] = s.d_sr[2];
                    s.d_sr[2] = s.d_sr[1];
                    s.d_sr[1] = input_sample_r;
                    input_sample_r = (s.d_sr[1] + s.d_sr[2] + s.d_sr[3]) / 3.0;
                    allpasstemp = s.alp_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.a_tr[allpasstemp as usize] * constallpass;
                    s.a_tr[s.alp_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_tr -= 1;
                    if s.alp_tr < 0 || s.alp_tr > s.delay_t {
                        s.alp_tr = s.delay_t;
                    }
                    input_sample_r += s.a_tr[s.alp_tr as usize];
                    s.d_tr[3] = s.d_tr[2];
                    s.d_tr[2] = s.d_tr[1];
                    s.d_tr[1] = input_sample_r;
                    input_sample_r = (s.d_tr[1] + s.d_tr[2] + s.d_tr[3]) / 3.0;
                    allpasstemp = s.alp_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.a_ur[allpasstemp as usize] * constallpass;
                    s.a_ur[s.alp_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ur -= 1;
                    if s.alp_ur < 0 || s.alp_ur > s.delay_u {
                        s.alp_ur = s.delay_u;
                    }
                    input_sample_r += s.a_ur[s.alp_ur as usize];
                    s.d_ur[3] = s.d_ur[2];
                    s.d_ur[2] = s.d_ur[1];
                    s.d_ur[1] = input_sample_r;
                    input_sample_r = (s.d_ur[1] + s.d_ur[2] + s.d_ur[3]) / 3.0;
                    allpasstemp = s.alp_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.a_vr[allpasstemp as usize] * constallpass;
                    s.a_vr[s.alp_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_vr -= 1;
                    if s.alp_vr < 0 || s.alp_vr > s.delay_v {
                        s.alp_vr = s.delay_v;
                    }
                    input_sample_r += s.a_vr[s.alp_vr as usize];
                    s.d_vr[3] = s.d_vr[2];
                    s.d_vr[2] = s.d_vr[1];
                    s.d_vr[1] = input_sample_r;
                    input_sample_r = (s.d_vr[1] + s.d_vr[2] + s.d_vr[3]) / 3.0;
                    allpasstemp = s.alp_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.a_wr[allpasstemp as usize] * constallpass;
                    s.a_wr[s.alp_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_wr -= 1;
                    if s.alp_wr < 0 || s.alp_wr > s.delay_w {
                        s.alp_wr = s.delay_w;
                    }
                    input_sample_r += s.a_wr[s.alp_wr as usize];
                    s.d_wr[3] = s.d_wr[2];
                    s.d_wr[2] = s.d_wr[1];
                    s.d_wr[1] = input_sample_r;
                    input_sample_r = (s.d_wr[1] + s.d_wr[2] + s.d_wr[3]) / 3.0;
                    allpasstemp = s.alp_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.a_xr[allpasstemp as usize] * constallpass;
                    s.a_xr[s.alp_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_xr -= 1;
                    if s.alp_xr < 0 || s.alp_xr > s.delay_x {
                        s.alp_xr = s.delay_x;
                    }
                    input_sample_r += s.a_xr[s.alp_xr as usize];
                    s.d_xr[3] = s.d_xr[2];
                    s.d_xr[2] = s.d_xr[1];
                    s.d_xr[1] = input_sample_r;
                    input_sample_r = (s.d_xr[1] + s.d_xr[2] + s.d_xr[3]) / 3.0;
                    allpasstemp = s.alp_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.a_yr[allpasstemp as usize] * constallpass;
                    s.a_yr[s.alp_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_yr -= 1;
                    if s.alp_yr < 0 || s.alp_yr > s.delay_y {
                        s.alp_yr = s.delay_y;
                    }
                    input_sample_r += s.a_yr[s.alp_yr as usize];
                    s.d_yr[3] = s.d_yr[2];
                    s.d_yr[2] = s.d_yr[1];
                    s.d_yr[1] = input_sample_r;
                    input_sample_r = (s.d_yr[1] + s.d_yr[2] + s.d_yr[3]) / 3.0;
                    allpasstemp = s.alp_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.a_zr[allpasstemp as usize] * constallpass;
                    s.a_zr[s.alp_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_zr -= 1;
                    if s.alp_zr < 0 || s.alp_zr > s.delay_z {
                        s.alp_zr = s.delay_z;
                    }
                    input_sample_r += s.a_zr[s.alp_zr as usize];
                    s.d_zr[3] = s.d_zr[2];
                    s.d_zr[2] = s.d_zr[1];
                    s.d_zr[1] = input_sample_r;
                    input_sample_r = (s.d_zr[1] + s.d_zr[2] + s.d_zr[3]) / 3.0;
                    allpasstemp = s.out_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.o_ar[allpasstemp as usize] * constallpass;
                    s.o_ar[s.out_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ar -= 1;
                    if s.out_ar < 0 || s.out_ar > s.delay_a {
                        s.out_ar = s.delay_a;
                    }
                    input_sample_r += s.o_ar[s.out_ar as usize];
                    s.d_ar[6] = s.d_ar[5];
                    s.d_ar[5] = s.d_ar[4];
                    s.d_ar[4] = input_sample_r;
                    input_sample_r = (s.d_yr[1] + s.d_ar[5] + s.d_ar[6]) / 3.0;
                    allpasstemp = s.out_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.o_br[allpasstemp as usize] * constallpass;
                    s.o_br[s.out_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_br -= 1;
                    if s.out_br < 0 || s.out_br > s.delay_b {
                        s.out_br = s.delay_b;
                    }
                    input_sample_r += s.o_br[s.out_br as usize];
                    s.d_br[6] = s.d_br[5];
                    s.d_br[5] = s.d_br[4];
                    s.d_br[4] = input_sample_r;
                    input_sample_r = (s.d_xr[1] + s.d_br[5] + s.d_br[6]) / 3.0;
                    allpasstemp = s.out_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.o_cr[allpasstemp as usize] * constallpass;
                    s.o_cr[s.out_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_cr -= 1;
                    if s.out_cr < 0 || s.out_cr > s.delay_c {
                        s.out_cr = s.delay_c;
                    }
                    input_sample_r += s.o_cr[s.out_cr as usize];
                    s.d_cr[6] = s.d_cr[5];
                    s.d_cr[5] = s.d_cr[4];
                    s.d_cr[4] = input_sample_r;
                    input_sample_r = (s.d_wr[1] + s.d_cr[5] + s.d_cr[6]) / 3.0;
                    allpasstemp = s.out_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.o_dr[allpasstemp as usize] * constallpass;
                    s.o_dr[s.out_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_dr -= 1;
                    if s.out_dr < 0 || s.out_dr > s.delay_d {
                        s.out_dr = s.delay_d;
                    }
                    input_sample_r += s.o_dr[s.out_dr as usize];
                    s.d_dr[6] = s.d_dr[5];
                    s.d_dr[5] = s.d_dr[4];
                    s.d_dr[4] = input_sample_r;
                    input_sample_r = (s.d_vr[1] + s.d_dr[5] + s.d_dr[6]) / 3.0;
                    allpasstemp = s.out_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.o_er[allpasstemp as usize] * constallpass;
                    s.o_er[s.out_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_er -= 1;
                    if s.out_er < 0 || s.out_er > s.delay_e {
                        s.out_er = s.delay_e;
                    }
                    input_sample_r += s.o_er[s.out_er as usize];
                    s.d_er[6] = s.d_er[5];
                    s.d_er[5] = s.d_er[4];
                    s.d_er[4] = input_sample_r;
                    input_sample_r = (s.d_ur[1] + s.d_er[5] + s.d_er[6]) / 3.0;
                    allpasstemp = s.out_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.o_fr[allpasstemp as usize] * constallpass;
                    s.o_fr[s.out_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_fr -= 1;
                    if s.out_fr < 0 || s.out_fr > s.delay_f {
                        s.out_fr = s.delay_f;
                    }
                    input_sample_r += s.o_fr[s.out_fr as usize];
                    s.d_fr[6] = s.d_fr[5];
                    s.d_fr[5] = s.d_fr[4];
                    s.d_fr[4] = input_sample_r;
                    input_sample_r = (s.d_tr[1] + s.d_fr[5] + s.d_fr[6]) / 3.0;
                    allpasstemp = s.out_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.o_gr[allpasstemp as usize] * constallpass;
                    s.o_gr[s.out_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_gr -= 1;
                    if s.out_gr < 0 || s.out_gr > s.delay_g {
                        s.out_gr = s.delay_g;
                    }
                    input_sample_r += s.o_gr[s.out_gr as usize];
                    s.d_gr[6] = s.d_gr[5];
                    s.d_gr[5] = s.d_gr[4];
                    s.d_gr[4] = input_sample_r;
                    input_sample_r = (s.d_sr[1] + s.d_gr[5] + s.d_gr[6]) / 3.0;
                    allpasstemp = s.out_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.o_hr[allpasstemp as usize] * constallpass;
                    s.o_hr[s.out_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_hr -= 1;
                    if s.out_hr < 0 || s.out_hr > s.delay_h {
                        s.out_hr = s.delay_h;
                    }
                    input_sample_r += s.o_hr[s.out_hr as usize];
                    s.d_hr[6] = s.d_hr[5];
                    s.d_hr[5] = s.d_hr[4];
                    s.d_hr[4] = input_sample_r;
                    input_sample_r = (s.d_rr[1] + s.d_hr[5] + s.d_hr[6]) / 3.0;
                    allpasstemp = s.out_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.o_ir[allpasstemp as usize] * constallpass;
                    s.o_ir[s.out_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ir -= 1;
                    if s.out_ir < 0 || s.out_ir > s.delay_i {
                        s.out_ir = s.delay_i;
                    }
                    input_sample_r += s.o_ir[s.out_ir as usize];
                    s.d_ir[6] = s.d_ir[5];
                    s.d_ir[5] = s.d_ir[4];
                    s.d_ir[4] = input_sample_r;
                    input_sample_r = (s.d_qr[1] + s.d_ir[5] + s.d_ir[6]) / 3.0;
                    allpasstemp = s.out_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.o_jr[allpasstemp as usize] * constallpass;
                    s.o_jr[s.out_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_jr -= 1;
                    if s.out_jr < 0 || s.out_jr > s.delay_j {
                        s.out_jr = s.delay_j;
                    }
                    input_sample_r += s.o_jr[s.out_jr as usize];
                    s.d_jr[6] = s.d_jr[5];
                    s.d_jr[5] = s.d_jr[4];
                    s.d_jr[4] = input_sample_r;
                    input_sample_r = (s.d_pr[1] + s.d_jr[5] + s.d_jr[6]) / 3.0;
                    allpasstemp = s.out_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.o_kr[allpasstemp as usize] * constallpass;
                    s.o_kr[s.out_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_kr -= 1;
                    if s.out_kr < 0 || s.out_kr > s.delay_k {
                        s.out_kr = s.delay_k;
                    }
                    input_sample_r += s.o_kr[s.out_kr as usize];
                    s.d_kr[6] = s.d_kr[5];
                    s.d_kr[5] = s.d_kr[4];
                    s.d_kr[4] = input_sample_r;
                    input_sample_r = (s.d_or[1] + s.d_kr[5] + s.d_kr[6]) / 3.0;
                    allpasstemp = s.out_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.o_lr[allpasstemp as usize] * constallpass;
                    s.o_lr[s.out_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_lr -= 1;
                    if s.out_lr < 0 || s.out_lr > s.delay_l {
                        s.out_lr = s.delay_l;
                    }
                    input_sample_r += s.o_lr[s.out_lr as usize];
                    s.d_lr[6] = s.d_lr[5];
                    s.d_lr[5] = s.d_lr[4];
                    s.d_lr[4] = input_sample_r;
                    input_sample_r = (s.d_nr[1] + s.d_lr[5] + s.d_lr[6]) / 3.0;
                    allpasstemp = s.out_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.o_mr[allpasstemp as usize] * constallpass;
                    s.o_mr[s.out_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_mr -= 1;
                    if s.out_mr < 0 || s.out_mr > s.delay_m {
                        s.out_mr = s.delay_m;
                    }
                    input_sample_r += s.o_mr[s.out_mr as usize];
                    s.d_mr[6] = s.d_mr[5];
                    s.d_mr[5] = s.d_mr[4];
                    s.d_mr[4] = input_sample_r;
                    input_sample_r = (s.d_mr[1] + s.d_mr[5] + s.d_mr[6]) / 3.0;
                    allpasstemp = s.out_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.o_nr[allpasstemp as usize] * constallpass;
                    s.o_nr[s.out_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_nr -= 1;
                    if s.out_nr < 0 || s.out_nr > s.delay_n {
                        s.out_nr = s.delay_n;
                    }
                    input_sample_r += s.o_nr[s.out_nr as usize];
                    s.d_nr[6] = s.d_nr[5];
                    s.d_nr[5] = s.d_nr[4];
                    s.d_nr[4] = input_sample_r;
                    input_sample_r = (s.d_lr[1] + s.d_nr[5] + s.d_nr[6]) / 3.0;
                    allpasstemp = s.out_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.o_or[allpasstemp as usize] * constallpass;
                    s.o_or[s.out_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_or -= 1;
                    if s.out_or < 0 || s.out_or > s.delay_o {
                        s.out_or = s.delay_o;
                    }
                    input_sample_r += s.o_or[s.out_or as usize];
                    s.d_or[6] = s.d_or[5];
                    s.d_or[5] = s.d_or[4];
                    s.d_or[4] = input_sample_r;
                    input_sample_r = (s.d_kr[1] + s.d_or[5] + s.d_or[6]) / 3.0;
                    allpasstemp = s.out_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.o_pr[allpasstemp as usize] * constallpass;
                    s.o_pr[s.out_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_pr -= 1;
                    if s.out_pr < 0 || s.out_pr > s.delay_p {
                        s.out_pr = s.delay_p;
                    }
                    input_sample_r += s.o_pr[s.out_pr as usize];
                    s.d_pr[6] = s.d_pr[5];
                    s.d_pr[5] = s.d_pr[4];
                    s.d_pr[4] = input_sample_r;
                    input_sample_r = (s.d_jr[1] + s.d_pr[5] + s.d_pr[6]) / 3.0;
                    allpasstemp = s.out_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.o_qr[allpasstemp as usize] * constallpass;
                    s.o_qr[s.out_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_qr -= 1;
                    if s.out_qr < 0 || s.out_qr > s.delay_q {
                        s.out_qr = s.delay_q;
                    }
                    input_sample_r += s.o_qr[s.out_qr as usize];
                    s.d_qr[6] = s.d_qr[5];
                    s.d_qr[5] = s.d_qr[4];
                    s.d_qr[4] = input_sample_r;
                    input_sample_r = (s.d_ir[1] + s.d_qr[5] + s.d_qr[6]) / 3.0;
                    allpasstemp = s.out_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.o_rr[allpasstemp as usize] * constallpass;
                    s.o_rr[s.out_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_rr -= 1;
                    if s.out_rr < 0 || s.out_rr > s.delay_r {
                        s.out_rr = s.delay_r;
                    }
                    input_sample_r += s.o_rr[s.out_rr as usize];
                    s.d_rr[6] = s.d_rr[5];
                    s.d_rr[5] = s.d_rr[4];
                    s.d_rr[4] = input_sample_r;
                    input_sample_r = (s.d_hr[1] + s.d_rr[5] + s.d_rr[6]) / 3.0;
                    allpasstemp = s.out_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.o_sr[allpasstemp as usize] * constallpass;
                    s.o_sr[s.out_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_sr -= 1;
                    if s.out_sr < 0 || s.out_sr > s.delay_s {
                        s.out_sr = s.delay_s;
                    }
                    input_sample_r += s.o_sr[s.out_sr as usize];
                    s.d_sr[6] = s.d_sr[5];
                    s.d_sr[5] = s.d_sr[4];
                    s.d_sr[4] = input_sample_r;
                    input_sample_r = (s.d_gr[1] + s.d_sr[5] + s.d_sr[6]) / 3.0;
                    allpasstemp = s.out_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.o_tr[allpasstemp as usize] * constallpass;
                    s.o_tr[s.out_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_tr -= 1;
                    if s.out_tr < 0 || s.out_tr > s.delay_t {
                        s.out_tr = s.delay_t;
                    }
                    input_sample_r += s.o_tr[s.out_tr as usize];
                    s.d_tr[6] = s.d_tr[5];
                    s.d_tr[5] = s.d_tr[4];
                    s.d_tr[4] = input_sample_r;
                    input_sample_r = (s.d_fr[1] + s.d_tr[5] + s.d_tr[6]) / 3.0;
                    allpasstemp = s.out_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.o_ur[allpasstemp as usize] * constallpass;
                    s.o_ur[s.out_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ur -= 1;
                    if s.out_ur < 0 || s.out_ur > s.delay_u {
                        s.out_ur = s.delay_u;
                    }
                    input_sample_r += s.o_ur[s.out_ur as usize];
                    s.d_ur[6] = s.d_ur[5];
                    s.d_ur[5] = s.d_ur[4];
                    s.d_ur[4] = input_sample_r;
                    input_sample_r = (s.d_er[1] + s.d_ur[5] + s.d_ur[6]) / 3.0;
                    allpasstemp = s.out_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.o_vr[allpasstemp as usize] * constallpass;
                    s.o_vr[s.out_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_vr -= 1;
                    if s.out_vr < 0 || s.out_vr > s.delay_v {
                        s.out_vr = s.delay_v;
                    }
                    input_sample_r += s.o_vr[s.out_vr as usize];
                    s.d_vr[6] = s.d_vr[5];
                    s.d_vr[5] = s.d_vr[4];
                    s.d_vr[4] = input_sample_r;
                    input_sample_r = (s.d_dr[1] + s.d_vr[5] + s.d_vr[6]) / 3.0;
                    allpasstemp = s.out_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.o_wr[allpasstemp as usize] * constallpass;
                    s.o_wr[s.out_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_wr -= 1;
                    if s.out_wr < 0 || s.out_wr > s.delay_w {
                        s.out_wr = s.delay_w;
                    }
                    input_sample_r += s.o_wr[s.out_wr as usize];
                    s.d_wr[6] = s.d_wr[5];
                    s.d_wr[5] = s.d_wr[4];
                    s.d_wr[4] = input_sample_r;
                    input_sample_r = (s.d_cr[1] + s.d_wr[5] + s.d_wr[6]) / 3.0;
                    allpasstemp = s.out_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.o_xr[allpasstemp as usize] * constallpass;
                    s.o_xr[s.out_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_xr -= 1;
                    if s.out_xr < 0 || s.out_xr > s.delay_x {
                        s.out_xr = s.delay_x;
                    }
                    input_sample_r += s.o_xr[s.out_xr as usize];
                    s.d_xr[6] = s.d_xr[5];
                    s.d_xr[5] = s.d_xr[4];
                    s.d_xr[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_xr[5] + s.d_xr[6]) / 3.0;
                    allpasstemp = s.out_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.o_yr[allpasstemp as usize] * constallpass;
                    s.o_yr[s.out_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_yr -= 1;
                    if s.out_yr < 0 || s.out_yr > s.delay_y {
                        s.out_yr = s.delay_y;
                    }
                    input_sample_r += s.o_yr[s.out_yr as usize];
                    s.d_yr[6] = s.d_yr[5];
                    s.d_yr[5] = s.d_yr[4];
                    s.d_yr[4] = input_sample_r;
                    input_sample_r = (s.d_br[1] + s.d_yr[5] + s.d_yr[6]) / 3.0;
                    allpasstemp = s.out_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.o_zr[allpasstemp as usize] * constallpass;
                    s.o_zr[s.out_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_zr -= 1;
                    if s.out_zr < 0 || s.out_zr > s.delay_z {
                        s.out_zr = s.delay_z;
                    }
                    input_sample_r += s.o_zr[s.out_zr as usize];
                    s.d_zr[6] = s.d_zr[5];
                    s.d_zr[5] = s.d_zr[4];
                    s.d_zr[4] = input_sample_r;
                    input_sample_r = s.d_zr[5] + s.d_zr[6];
                }
                3 => {
                    allpasstemp = s.alp_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.a_ar[allpasstemp as usize] * constallpass;
                    s.a_ar[s.alp_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ar -= 1;
                    if s.alp_ar < 0 || s.alp_ar > s.delay_a {
                        s.alp_ar = s.delay_a;
                    }
                    input_sample_r += s.a_ar[s.alp_ar as usize];
                    s.d_ar[2] = s.d_ar[1];
                    s.d_ar[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_ar[2]) / 2.0;
                    allpasstemp = s.alp_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.a_br[allpasstemp as usize] * constallpass;
                    s.a_br[s.alp_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_br -= 1;
                    if s.alp_br < 0 || s.alp_br > s.delay_b {
                        s.alp_br = s.delay_b;
                    }
                    input_sample_r += s.a_br[s.alp_br as usize];
                    s.d_br[2] = s.d_br[1];
                    s.d_br[1] = input_sample_r;
                    input_sample_r = (s.d_br[1] + s.d_br[2]) / 2.0;
                    allpasstemp = s.alp_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.a_cr[allpasstemp as usize] * constallpass;
                    s.a_cr[s.alp_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_cr -= 1;
                    if s.alp_cr < 0 || s.alp_cr > s.delay_c {
                        s.alp_cr = s.delay_c;
                    }
                    input_sample_r += s.a_cr[s.alp_cr as usize];
                    s.d_cr[2] = s.d_cr[1];
                    s.d_cr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_cr[2]) / 2.0;
                    allpasstemp = s.alp_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.a_dr[allpasstemp as usize] * constallpass;
                    s.a_dr[s.alp_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_dr -= 1;
                    if s.alp_dr < 0 || s.alp_dr > s.delay_d {
                        s.alp_dr = s.delay_d;
                    }
                    input_sample_r += s.a_dr[s.alp_dr as usize];
                    s.d_dr[2] = s.d_dr[1];
                    s.d_dr[1] = input_sample_r;
                    input_sample_r = (s.d_dr[1] + s.d_dr[2]) / 2.0;
                    allpasstemp = s.alp_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.a_er[allpasstemp as usize] * constallpass;
                    s.a_er[s.alp_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_er -= 1;
                    if s.alp_er < 0 || s.alp_er > s.delay_e {
                        s.alp_er = s.delay_e;
                    }
                    input_sample_r += s.a_er[s.alp_er as usize];
                    s.d_er[2] = s.d_er[1];
                    s.d_er[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_er[2]) / 2.0;
                    allpasstemp = s.alp_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.a_fr[allpasstemp as usize] * constallpass;
                    s.a_fr[s.alp_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_fr -= 1;
                    if s.alp_fr < 0 || s.alp_fr > s.delay_f {
                        s.alp_fr = s.delay_f;
                    }
                    input_sample_r += s.a_fr[s.alp_fr as usize];
                    s.d_fr[2] = s.d_fr[1];
                    s.d_fr[1] = input_sample_r;
                    input_sample_r = (s.d_fr[1] + s.d_fr[2]) / 2.0;
                    allpasstemp = s.alp_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.a_gr[allpasstemp as usize] * constallpass;
                    s.a_gr[s.alp_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_gr -= 1;
                    if s.alp_gr < 0 || s.alp_gr > s.delay_g {
                        s.alp_gr = s.delay_g;
                    }
                    input_sample_r += s.a_gr[s.alp_gr as usize];
                    s.d_gr[2] = s.d_gr[1];
                    s.d_gr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_gr[2]) / 2.0;
                    allpasstemp = s.alp_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.a_hr[allpasstemp as usize] * constallpass;
                    s.a_hr[s.alp_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_hr -= 1;
                    if s.alp_hr < 0 || s.alp_hr > s.delay_h {
                        s.alp_hr = s.delay_h;
                    }
                    input_sample_r += s.a_hr[s.alp_hr as usize];
                    s.d_hr[2] = s.d_hr[1];
                    s.d_hr[1] = input_sample_r;
                    input_sample_r = (s.d_hr[1] + s.d_hr[2]) / 2.0;
                    allpasstemp = s.alp_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.a_ir[allpasstemp as usize] * constallpass;
                    s.a_ir[s.alp_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ir -= 1;
                    if s.alp_ir < 0 || s.alp_ir > s.delay_i {
                        s.alp_ir = s.delay_i;
                    }
                    input_sample_r += s.a_ir[s.alp_ir as usize];
                    s.d_ir[2] = s.d_ir[1];
                    s.d_ir[1] = input_sample_r;
                    input_sample_r = (s.d_ir[1] + s.d_ir[2]) / 2.0;
                    allpasstemp = s.alp_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.a_jr[allpasstemp as usize] * constallpass;
                    s.a_jr[s.alp_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_jr -= 1;
                    if s.alp_jr < 0 || s.alp_jr > s.delay_j {
                        s.alp_jr = s.delay_j;
                    }
                    input_sample_r += s.a_jr[s.alp_jr as usize];
                    s.d_jr[2] = s.d_jr[1];
                    s.d_jr[1] = input_sample_r;
                    input_sample_r = (s.d_jr[1] + s.d_jr[2]) / 2.0;
                    allpasstemp = s.alp_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.a_kr[allpasstemp as usize] * constallpass;
                    s.a_kr[s.alp_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_kr -= 1;
                    if s.alp_kr < 0 || s.alp_kr > s.delay_k {
                        s.alp_kr = s.delay_k;
                    }
                    input_sample_r += s.a_kr[s.alp_kr as usize];
                    s.d_kr[2] = s.d_kr[1];
                    s.d_kr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_kr[2]) / 2.0;
                    allpasstemp = s.alp_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.a_lr[allpasstemp as usize] * constallpass;
                    s.a_lr[s.alp_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_lr -= 1;
                    if s.alp_lr < 0 || s.alp_lr > s.delay_l {
                        s.alp_lr = s.delay_l;
                    }
                    input_sample_r += s.a_lr[s.alp_lr as usize];
                    s.d_lr[2] = s.d_lr[1];
                    s.d_lr[1] = input_sample_r;
                    input_sample_r = (s.d_lr[1] + s.d_lr[2]) / 2.0;
                    allpasstemp = s.alp_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.a_mr[allpasstemp as usize] * constallpass;
                    s.a_mr[s.alp_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_mr -= 1;
                    if s.alp_mr < 0 || s.alp_mr > s.delay_m {
                        s.alp_mr = s.delay_m;
                    }
                    input_sample_r += s.a_mr[s.alp_mr as usize];
                    s.d_mr[2] = s.d_mr[1];
                    s.d_mr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_mr[2]) / 2.0;
                    allpasstemp = s.alp_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.a_nr[allpasstemp as usize] * constallpass;
                    s.a_nr[s.alp_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_nr -= 1;
                    if s.alp_nr < 0 || s.alp_nr > s.delay_n {
                        s.alp_nr = s.delay_n;
                    }
                    input_sample_r += s.a_nr[s.alp_nr as usize];
                    s.d_nr[2] = s.d_nr[1];
                    s.d_nr[1] = input_sample_r;
                    input_sample_r = (s.d_nr[1] + s.d_nr[2]) / 2.0;
                    allpasstemp = s.alp_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.a_or[allpasstemp as usize] * constallpass;
                    s.a_or[s.alp_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_or -= 1;
                    if s.alp_or < 0 || s.alp_or > s.delay_o {
                        s.alp_or = s.delay_o;
                    }
                    input_sample_r += s.a_or[s.alp_or as usize];
                    s.d_or[2] = s.d_or[1];
                    s.d_or[1] = input_sample_r;
                    input_sample_r = (s.d_or[1] + s.d_or[2]) / 2.0;
                    allpasstemp = s.alp_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.a_pr[allpasstemp as usize] * constallpass;
                    s.a_pr[s.alp_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_pr -= 1;
                    if s.alp_pr < 0 || s.alp_pr > s.delay_p {
                        s.alp_pr = s.delay_p;
                    }
                    input_sample_r += s.a_pr[s.alp_pr as usize];
                    s.d_pr[2] = s.d_pr[1];
                    s.d_pr[1] = input_sample_r;
                    input_sample_r = (s.d_pr[1] + s.d_pr[2]) / 2.0;
                    allpasstemp = s.alp_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.a_qr[allpasstemp as usize] * constallpass;
                    s.a_qr[s.alp_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_qr -= 1;
                    if s.alp_qr < 0 || s.alp_qr > s.delay_q {
                        s.alp_qr = s.delay_q;
                    }
                    input_sample_r += s.a_qr[s.alp_qr as usize];
                    s.d_qr[2] = s.d_qr[1];
                    s.d_qr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_qr[2]) / 2.0;
                    allpasstemp = s.alp_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.a_rr[allpasstemp as usize] * constallpass;
                    s.a_rr[s.alp_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_rr -= 1;
                    if s.alp_rr < 0 || s.alp_rr > s.delay_r {
                        s.alp_rr = s.delay_r;
                    }
                    input_sample_r += s.a_rr[s.alp_rr as usize];
                    s.d_rr[2] = s.d_rr[1];
                    s.d_rr[1] = input_sample_r;
                    input_sample_r = (s.d_rr[1] + s.d_rr[2]) / 2.0;
                    allpasstemp = s.alp_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.a_sr[allpasstemp as usize] * constallpass;
                    s.a_sr[s.alp_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_sr -= 1;
                    if s.alp_sr < 0 || s.alp_sr > s.delay_s {
                        s.alp_sr = s.delay_s;
                    }
                    input_sample_r += s.a_sr[s.alp_sr as usize];
                    s.d_sr[2] = s.d_sr[1];
                    s.d_sr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_sr[2]) / 2.0;
                    allpasstemp = s.alp_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.a_tr[allpasstemp as usize] * constallpass;
                    s.a_tr[s.alp_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_tr -= 1;
                    if s.alp_tr < 0 || s.alp_tr > s.delay_t {
                        s.alp_tr = s.delay_t;
                    }
                    input_sample_r += s.a_tr[s.alp_tr as usize];
                    s.d_tr[2] = s.d_tr[1];
                    s.d_tr[1] = input_sample_r;
                    input_sample_r = (s.d_tr[1] + s.d_tr[2]) / 2.0;
                    allpasstemp = s.alp_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.a_ur[allpasstemp as usize] * constallpass;
                    s.a_ur[s.alp_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ur -= 1;
                    if s.alp_ur < 0 || s.alp_ur > s.delay_u {
                        s.alp_ur = s.delay_u;
                    }
                    input_sample_r += s.a_ur[s.alp_ur as usize];
                    s.d_ur[2] = s.d_ur[1];
                    s.d_ur[1] = input_sample_r;
                    input_sample_r = (s.d_ur[1] + s.d_ur[2]) / 2.0;
                    allpasstemp = s.alp_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.a_vr[allpasstemp as usize] * constallpass;
                    s.a_vr[s.alp_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_vr -= 1;
                    if s.alp_vr < 0 || s.alp_vr > s.delay_v {
                        s.alp_vr = s.delay_v;
                    }
                    input_sample_r += s.a_vr[s.alp_vr as usize];
                    s.d_vr[2] = s.d_vr[1];
                    s.d_vr[1] = input_sample_r;
                    input_sample_r = (s.d_vr[1] + s.d_vr[2]) / 2.0;
                    allpasstemp = s.alp_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.a_wr[allpasstemp as usize] * constallpass;
                    s.a_wr[s.alp_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_wr -= 1;
                    if s.alp_wr < 0 || s.alp_wr > s.delay_w {
                        s.alp_wr = s.delay_w;
                    }
                    input_sample_r += s.a_wr[s.alp_wr as usize];
                    s.d_wr[2] = s.d_wr[1];
                    s.d_wr[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_wr[2]) / 2.0;
                    allpasstemp = s.alp_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.a_xr[allpasstemp as usize] * constallpass;
                    s.a_xr[s.alp_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_xr -= 1;
                    if s.alp_xr < 0 || s.alp_xr > s.delay_x {
                        s.alp_xr = s.delay_x;
                    }
                    input_sample_r += s.a_xr[s.alp_xr as usize];
                    s.d_xr[2] = s.d_xr[1];
                    s.d_xr[1] = input_sample_r;
                    input_sample_r = (s.d_xr[1] + s.d_xr[2]) / 2.0;
                    allpasstemp = s.alp_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.a_yr[allpasstemp as usize] * constallpass;
                    s.a_yr[s.alp_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_yr -= 1;
                    if s.alp_yr < 0 || s.alp_yr > s.delay_y {
                        s.alp_yr = s.delay_y;
                    }
                    input_sample_r += s.a_yr[s.alp_yr as usize];
                    s.d_yr[2] = s.d_yr[1];
                    s.d_yr[1] = input_sample_r;
                    input_sample_r = (s.d_yr[1] + s.d_yr[2]) / 2.0;
                    allpasstemp = s.alp_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.a_zr[allpasstemp as usize] * constallpass;
                    s.a_zr[s.alp_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_zr -= 1;
                    if s.alp_zr < 0 || s.alp_zr > s.delay_z {
                        s.alp_zr = s.delay_z;
                    }
                    input_sample_r += s.a_zr[s.alp_zr as usize];
                    s.d_zr[2] = s.d_zr[1];
                    s.d_zr[1] = input_sample_r;
                    input_sample_r = (s.d_zr[1] + s.d_zr[2]) / 2.0;
                    allpasstemp = s.out_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.o_ar[allpasstemp as usize] * constallpass;
                    s.o_ar[s.out_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ar -= 1;
                    if s.out_ar < 0 || s.out_ar > s.delay_a {
                        s.out_ar = s.delay_a;
                    }
                    input_sample_r += s.o_ar[s.out_ar as usize];
                    s.d_ar[5] = s.d_ar[4];
                    s.d_ar[4] = input_sample_r;
                    input_sample_r = (s.d_ar[4] + s.d_ar[5]) / 2.0;
                    allpasstemp = s.out_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.o_br[allpasstemp as usize] * constallpass;
                    s.o_br[s.out_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_br -= 1;
                    if s.out_br < 0 || s.out_br > s.delay_b {
                        s.out_br = s.delay_b;
                    }
                    input_sample_r += s.o_br[s.out_br as usize];
                    s.d_br[5] = s.d_br[4];
                    s.d_br[4] = input_sample_r;
                    input_sample_r = (s.d_br[4] + s.d_br[5]) / 2.0;
                    allpasstemp = s.out_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.o_cr[allpasstemp as usize] * constallpass;
                    s.o_cr[s.out_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_cr -= 1;
                    if s.out_cr < 0 || s.out_cr > s.delay_c {
                        s.out_cr = s.delay_c;
                    }
                    input_sample_r += s.o_cr[s.out_cr as usize];
                    s.d_cr[5] = s.d_cr[4];
                    s.d_cr[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_cr[5]) / 2.0;
                    allpasstemp = s.out_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.o_dr[allpasstemp as usize] * constallpass;
                    s.o_dr[s.out_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_dr -= 1;
                    if s.out_dr < 0 || s.out_dr > s.delay_d {
                        s.out_dr = s.delay_d;
                    }
                    input_sample_r += s.o_dr[s.out_dr as usize];
                    s.d_dr[5] = s.d_dr[4];
                    s.d_dr[4] = input_sample_r;
                    input_sample_r = (s.d_dr[4] + s.d_dr[5]) / 2.0;
                    allpasstemp = s.out_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.o_er[allpasstemp as usize] * constallpass;
                    s.o_er[s.out_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_er -= 1;
                    if s.out_er < 0 || s.out_er > s.delay_e {
                        s.out_er = s.delay_e;
                    }
                    input_sample_r += s.o_er[s.out_er as usize];
                    s.d_er[5] = s.d_er[4];
                    s.d_er[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_er[5]) / 2.0;
                    allpasstemp = s.out_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.o_fr[allpasstemp as usize] * constallpass;
                    s.o_fr[s.out_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_fr -= 1;
                    if s.out_fr < 0 || s.out_fr > s.delay_f {
                        s.out_fr = s.delay_f;
                    }
                    input_sample_r += s.o_fr[s.out_fr as usize];
                    s.d_fr[5] = s.d_fr[4];
                    s.d_fr[4] = input_sample_r;
                    input_sample_r = (s.d_fr[4] + s.d_fr[5]) / 2.0;
                    allpasstemp = s.out_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.o_gr[allpasstemp as usize] * constallpass;
                    s.o_gr[s.out_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_gr -= 1;
                    if s.out_gr < 0 || s.out_gr > s.delay_g {
                        s.out_gr = s.delay_g;
                    }
                    input_sample_r += s.o_gr[s.out_gr as usize];
                    s.d_gr[5] = s.d_gr[4];
                    s.d_gr[4] = input_sample_r;
                    input_sample_r = (s.d_gr[4] + s.d_gr[5]) / 2.0;
                    allpasstemp = s.out_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.o_hr[allpasstemp as usize] * constallpass;
                    s.o_hr[s.out_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_hr -= 1;
                    if s.out_hr < 0 || s.out_hr > s.delay_h {
                        s.out_hr = s.delay_h;
                    }
                    input_sample_r += s.o_hr[s.out_hr as usize];
                    s.d_hr[5] = s.d_hr[4];
                    s.d_hr[4] = input_sample_r;
                    input_sample_r = (s.d_hr[4] + s.d_hr[5]) / 2.0;
                    allpasstemp = s.out_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.o_ir[allpasstemp as usize] * constallpass;
                    s.o_ir[s.out_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ir -= 1;
                    if s.out_ir < 0 || s.out_ir > s.delay_i {
                        s.out_ir = s.delay_i;
                    }
                    input_sample_r += s.o_ir[s.out_ir as usize];
                    s.d_ir[5] = s.d_ir[4];
                    s.d_ir[4] = input_sample_r;
                    input_sample_r = (s.d_ir[4] + s.d_ir[5]) / 2.0;
                    allpasstemp = s.out_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.o_jr[allpasstemp as usize] * constallpass;
                    s.o_jr[s.out_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_jr -= 1;
                    if s.out_jr < 0 || s.out_jr > s.delay_j {
                        s.out_jr = s.delay_j;
                    }
                    input_sample_r += s.o_jr[s.out_jr as usize];
                    s.d_jr[5] = s.d_jr[4];
                    s.d_jr[4] = input_sample_r;
                    input_sample_r = (s.d_jr[4] + s.d_jr[5]) / 2.0;
                    allpasstemp = s.out_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.o_kr[allpasstemp as usize] * constallpass;
                    s.o_kr[s.out_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_kr -= 1;
                    if s.out_kr < 0 || s.out_kr > s.delay_k {
                        s.out_kr = s.delay_k;
                    }
                    input_sample_r += s.o_kr[s.out_kr as usize];
                    s.d_kr[5] = s.d_kr[4];
                    s.d_kr[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_kr[5]) / 2.0;
                    allpasstemp = s.out_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.o_lr[allpasstemp as usize] * constallpass;
                    s.o_lr[s.out_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_lr -= 1;
                    if s.out_lr < 0 || s.out_lr > s.delay_l {
                        s.out_lr = s.delay_l;
                    }
                    input_sample_r += s.o_lr[s.out_lr as usize];
                    s.d_lr[5] = s.d_lr[4];
                    s.d_lr[4] = input_sample_r;
                    input_sample_r = (s.d_lr[4] + s.d_lr[5]) / 2.0;
                    allpasstemp = s.out_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.o_mr[allpasstemp as usize] * constallpass;
                    s.o_mr[s.out_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_mr -= 1;
                    if s.out_mr < 0 || s.out_mr > s.delay_m {
                        s.out_mr = s.delay_m;
                    }
                    input_sample_r += s.o_mr[s.out_mr as usize];
                    s.d_mr[5] = s.d_mr[4];
                    s.d_mr[4] = input_sample_r;
                    input_sample_r = (s.d_mr[4] + s.d_mr[5]) / 2.0;
                    allpasstemp = s.out_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.o_nr[allpasstemp as usize] * constallpass;
                    s.o_nr[s.out_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_nr -= 1;
                    if s.out_nr < 0 || s.out_nr > s.delay_n {
                        s.out_nr = s.delay_n;
                    }
                    input_sample_r += s.o_nr[s.out_nr as usize];
                    s.d_nr[5] = s.d_nr[4];
                    s.d_nr[4] = input_sample_r;
                    input_sample_r = (s.d_nr[4] + s.d_nr[5]) / 2.0;
                    allpasstemp = s.out_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.o_or[allpasstemp as usize] * constallpass;
                    s.o_or[s.out_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_or -= 1;
                    if s.out_or < 0 || s.out_or > s.delay_o {
                        s.out_or = s.delay_o;
                    }
                    input_sample_r += s.o_or[s.out_or as usize];
                    s.d_or[5] = s.d_or[4];
                    s.d_or[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_or[5]) / 2.0;
                    allpasstemp = s.out_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.o_pr[allpasstemp as usize] * constallpass;
                    s.o_pr[s.out_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_pr -= 1;
                    if s.out_pr < 0 || s.out_pr > s.delay_p {
                        s.out_pr = s.delay_p;
                    }
                    input_sample_r += s.o_pr[s.out_pr as usize];
                    s.d_pr[5] = s.d_pr[4];
                    s.d_pr[4] = input_sample_r;
                    input_sample_r = (s.d_pr[4] + s.d_pr[5]) / 2.0;
                    allpasstemp = s.out_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.o_qr[allpasstemp as usize] * constallpass;
                    s.o_qr[s.out_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_qr -= 1;
                    if s.out_qr < 0 || s.out_qr > s.delay_q {
                        s.out_qr = s.delay_q;
                    }
                    input_sample_r += s.o_qr[s.out_qr as usize];
                    s.d_qr[5] = s.d_qr[4];
                    s.d_qr[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_qr[5]) / 2.0;
                    allpasstemp = s.out_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.o_rr[allpasstemp as usize] * constallpass;
                    s.o_rr[s.out_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_rr -= 1;
                    if s.out_rr < 0 || s.out_rr > s.delay_r {
                        s.out_rr = s.delay_r;
                    }
                    input_sample_r += s.o_rr[s.out_rr as usize];
                    s.d_rr[5] = s.d_rr[4];
                    s.d_rr[4] = input_sample_r;
                    input_sample_r = (s.d_rr[4] + s.d_rr[5]) / 2.0;
                    allpasstemp = s.out_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.o_sr[allpasstemp as usize] * constallpass;
                    s.o_sr[s.out_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_sr -= 1;
                    if s.out_sr < 0 || s.out_sr > s.delay_s {
                        s.out_sr = s.delay_s;
                    }
                    input_sample_r += s.o_sr[s.out_sr as usize];
                    s.d_sr[5] = s.d_sr[4];
                    s.d_sr[4] = input_sample_r;
                    input_sample_r = (s.d_sr[4] + s.d_sr[5]) / 2.0;
                    allpasstemp = s.out_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.o_tr[allpasstemp as usize] * constallpass;
                    s.o_tr[s.out_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_tr -= 1;
                    if s.out_tr < 0 || s.out_tr > s.delay_t {
                        s.out_tr = s.delay_t;
                    }
                    input_sample_r += s.o_tr[s.out_tr as usize];
                    s.d_tr[5] = s.d_tr[4];
                    s.d_tr[4] = input_sample_r;
                    input_sample_r = (s.d_tr[4] + s.d_tr[5]) / 2.0;
                    allpasstemp = s.out_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.o_ur[allpasstemp as usize] * constallpass;
                    s.o_ur[s.out_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ur -= 1;
                    if s.out_ur < 0 || s.out_ur > s.delay_u {
                        s.out_ur = s.delay_u;
                    }
                    input_sample_r += s.o_ur[s.out_ur as usize];
                    s.d_ur[5] = s.d_ur[4];
                    s.d_ur[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_ur[5]) / 2.0;
                    allpasstemp = s.out_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.o_vr[allpasstemp as usize] * constallpass;
                    s.o_vr[s.out_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_vr -= 1;
                    if s.out_vr < 0 || s.out_vr > s.delay_v {
                        s.out_vr = s.delay_v;
                    }
                    input_sample_r += s.o_vr[s.out_vr as usize];
                    s.d_vr[5] = s.d_vr[4];
                    s.d_vr[4] = input_sample_r;
                    input_sample_r = (s.d_vr[4] + s.d_vr[5]) / 2.0;
                    allpasstemp = s.out_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.o_wr[allpasstemp as usize] * constallpass;
                    s.o_wr[s.out_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_wr -= 1;
                    if s.out_wr < 0 || s.out_wr > s.delay_w {
                        s.out_wr = s.delay_w;
                    }
                    input_sample_r += s.o_wr[s.out_wr as usize];
                    s.d_wr[5] = s.d_wr[4];
                    s.d_wr[4] = input_sample_r;
                    input_sample_r = (s.d_wr[4] + s.d_wr[5]) / 2.0;
                    allpasstemp = s.out_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.o_xr[allpasstemp as usize] * constallpass;
                    s.o_xr[s.out_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_xr -= 1;
                    if s.out_xr < 0 || s.out_xr > s.delay_x {
                        s.out_xr = s.delay_x;
                    }
                    input_sample_r += s.o_xr[s.out_xr as usize];
                    s.d_xr[5] = s.d_xr[4];
                    s.d_xr[4] = input_sample_r;
                    input_sample_r = (s.d_xr[4] + s.d_xr[5]) / 2.0;
                    allpasstemp = s.out_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.o_yr[allpasstemp as usize] * constallpass;
                    s.o_yr[s.out_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_yr -= 1;
                    if s.out_yr < 0 || s.out_yr > s.delay_y {
                        s.out_yr = s.delay_y;
                    }
                    input_sample_r += s.o_yr[s.out_yr as usize];
                    s.d_yr[5] = s.d_yr[4];
                    s.d_yr[4] = input_sample_r;
                    input_sample_r = (s.d_yr[4] + s.d_yr[5]) / 2.0;
                    allpasstemp = s.out_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.o_zr[allpasstemp as usize] * constallpass;
                    s.o_zr[s.out_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_zr -= 1;
                    if s.out_zr < 0 || s.out_zr > s.delay_z {
                        s.out_zr = s.delay_z;
                    }
                    input_sample_r += s.o_zr[s.out_zr as usize];
                    s.d_zr[5] = s.d_zr[4];
                    s.d_zr[4] = input_sample_r;
                    input_sample_r = s.d_zr[4] + s.d_zr[5];
                }
                4 => {
                    allpasstemp = s.alp_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.a_ar[allpasstemp as usize] * constallpass;
                    s.a_ar[s.alp_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ar -= 1;
                    if s.alp_ar < 0 || s.alp_ar > s.delay_a {
                        s.alp_ar = s.delay_a;
                    }
                    input_sample_r += s.a_ar[s.alp_ar as usize];
                    s.d_ar[2] = s.d_ar[1];
                    s.d_ar[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.a_br[allpasstemp as usize] * constallpass;
                    s.a_br[s.alp_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_br -= 1;
                    if s.alp_br < 0 || s.alp_br > s.delay_b {
                        s.alp_br = s.delay_b;
                    }
                    input_sample_r += s.a_br[s.alp_br as usize];
                    s.d_br[2] = s.d_br[1];
                    s.d_br[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.a_cr[allpasstemp as usize] * constallpass;
                    s.a_cr[s.alp_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_cr -= 1;
                    if s.alp_cr < 0 || s.alp_cr > s.delay_c {
                        s.alp_cr = s.delay_c;
                    }
                    input_sample_r += s.a_cr[s.alp_cr as usize];
                    s.d_cr[2] = s.d_cr[1];
                    s.d_cr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.a_dr[allpasstemp as usize] * constallpass;
                    s.a_dr[s.alp_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_dr -= 1;
                    if s.alp_dr < 0 || s.alp_dr > s.delay_d {
                        s.alp_dr = s.delay_d;
                    }
                    input_sample_r += s.a_dr[s.alp_dr as usize];
                    s.d_dr[2] = s.d_dr[1];
                    s.d_dr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.a_er[allpasstemp as usize] * constallpass;
                    s.a_er[s.alp_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_er -= 1;
                    if s.alp_er < 0 || s.alp_er > s.delay_e {
                        s.alp_er = s.delay_e;
                    }
                    input_sample_r += s.a_er[s.alp_er as usize];
                    s.d_er[2] = s.d_er[1];
                    s.d_er[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.a_fr[allpasstemp as usize] * constallpass;
                    s.a_fr[s.alp_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_fr -= 1;
                    if s.alp_fr < 0 || s.alp_fr > s.delay_f {
                        s.alp_fr = s.delay_f;
                    }
                    input_sample_r += s.a_fr[s.alp_fr as usize];
                    s.d_fr[2] = s.d_fr[1];
                    s.d_fr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.a_gr[allpasstemp as usize] * constallpass;
                    s.a_gr[s.alp_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_gr -= 1;
                    if s.alp_gr < 0 || s.alp_gr > s.delay_g {
                        s.alp_gr = s.delay_g;
                    }
                    input_sample_r += s.a_gr[s.alp_gr as usize];
                    s.d_gr[2] = s.d_gr[1];
                    s.d_gr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.a_hr[allpasstemp as usize] * constallpass;
                    s.a_hr[s.alp_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_hr -= 1;
                    if s.alp_hr < 0 || s.alp_hr > s.delay_h {
                        s.alp_hr = s.delay_h;
                    }
                    input_sample_r += s.a_hr[s.alp_hr as usize];
                    s.d_hr[2] = s.d_hr[1];
                    s.d_hr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.a_ir[allpasstemp as usize] * constallpass;
                    s.a_ir[s.alp_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ir -= 1;
                    if s.alp_ir < 0 || s.alp_ir > s.delay_i {
                        s.alp_ir = s.delay_i;
                    }
                    input_sample_r += s.a_ir[s.alp_ir as usize];
                    s.d_ir[2] = s.d_ir[1];
                    s.d_ir[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.a_jr[allpasstemp as usize] * constallpass;
                    s.a_jr[s.alp_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_jr -= 1;
                    if s.alp_jr < 0 || s.alp_jr > s.delay_j {
                        s.alp_jr = s.delay_j;
                    }
                    input_sample_r += s.a_jr[s.alp_jr as usize];
                    s.d_jr[2] = s.d_jr[1];
                    s.d_jr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.a_kr[allpasstemp as usize] * constallpass;
                    s.a_kr[s.alp_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_kr -= 1;
                    if s.alp_kr < 0 || s.alp_kr > s.delay_k {
                        s.alp_kr = s.delay_k;
                    }
                    input_sample_r += s.a_kr[s.alp_kr as usize];
                    s.d_kr[2] = s.d_kr[1];
                    s.d_kr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.a_lr[allpasstemp as usize] * constallpass;
                    s.a_lr[s.alp_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_lr -= 1;
                    if s.alp_lr < 0 || s.alp_lr > s.delay_l {
                        s.alp_lr = s.delay_l;
                    }
                    input_sample_r += s.a_lr[s.alp_lr as usize];
                    s.d_lr[2] = s.d_lr[1];
                    s.d_lr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.a_mr[allpasstemp as usize] * constallpass;
                    s.a_mr[s.alp_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_mr -= 1;
                    if s.alp_mr < 0 || s.alp_mr > s.delay_m {
                        s.alp_mr = s.delay_m;
                    }
                    input_sample_r += s.a_mr[s.alp_mr as usize];
                    s.d_mr[2] = s.d_mr[1];
                    s.d_mr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.a_nr[allpasstemp as usize] * constallpass;
                    s.a_nr[s.alp_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_nr -= 1;
                    if s.alp_nr < 0 || s.alp_nr > s.delay_n {
                        s.alp_nr = s.delay_n;
                    }
                    input_sample_r += s.a_nr[s.alp_nr as usize];
                    s.d_nr[2] = s.d_nr[1];
                    s.d_nr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.a_or[allpasstemp as usize] * constallpass;
                    s.a_or[s.alp_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_or -= 1;
                    if s.alp_or < 0 || s.alp_or > s.delay_o {
                        s.alp_or = s.delay_o;
                    }
                    input_sample_r += s.a_or[s.alp_or as usize];
                    s.d_or[2] = s.d_or[1];
                    s.d_or[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.a_pr[allpasstemp as usize] * constallpass;
                    s.a_pr[s.alp_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_pr -= 1;
                    if s.alp_pr < 0 || s.alp_pr > s.delay_p {
                        s.alp_pr = s.delay_p;
                    }
                    input_sample_r += s.a_pr[s.alp_pr as usize];
                    s.d_pr[2] = s.d_pr[1];
                    s.d_pr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.a_qr[allpasstemp as usize] * constallpass;
                    s.a_qr[s.alp_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_qr -= 1;
                    if s.alp_qr < 0 || s.alp_qr > s.delay_q {
                        s.alp_qr = s.delay_q;
                    }
                    input_sample_r += s.a_qr[s.alp_qr as usize];
                    s.d_qr[2] = s.d_qr[1];
                    s.d_qr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.a_rr[allpasstemp as usize] * constallpass;
                    s.a_rr[s.alp_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_rr -= 1;
                    if s.alp_rr < 0 || s.alp_rr > s.delay_r {
                        s.alp_rr = s.delay_r;
                    }
                    input_sample_r += s.a_rr[s.alp_rr as usize];
                    s.d_rr[2] = s.d_rr[1];
                    s.d_rr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.a_sr[allpasstemp as usize] * constallpass;
                    s.a_sr[s.alp_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_sr -= 1;
                    if s.alp_sr < 0 || s.alp_sr > s.delay_s {
                        s.alp_sr = s.delay_s;
                    }
                    input_sample_r += s.a_sr[s.alp_sr as usize];
                    s.d_sr[2] = s.d_sr[1];
                    s.d_sr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.a_tr[allpasstemp as usize] * constallpass;
                    s.a_tr[s.alp_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_tr -= 1;
                    if s.alp_tr < 0 || s.alp_tr > s.delay_t {
                        s.alp_tr = s.delay_t;
                    }
                    input_sample_r += s.a_tr[s.alp_tr as usize];
                    s.d_tr[2] = s.d_tr[1];
                    s.d_tr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.a_ur[allpasstemp as usize] * constallpass;
                    s.a_ur[s.alp_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ur -= 1;
                    if s.alp_ur < 0 || s.alp_ur > s.delay_u {
                        s.alp_ur = s.delay_u;
                    }
                    input_sample_r += s.a_ur[s.alp_ur as usize];
                    s.d_ur[2] = s.d_ur[1];
                    s.d_ur[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.a_vr[allpasstemp as usize] * constallpass;
                    s.a_vr[s.alp_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_vr -= 1;
                    if s.alp_vr < 0 || s.alp_vr > s.delay_v {
                        s.alp_vr = s.delay_v;
                    }
                    input_sample_r += s.a_vr[s.alp_vr as usize];
                    s.d_vr[2] = s.d_vr[1];
                    s.d_vr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.a_wr[allpasstemp as usize] * constallpass;
                    s.a_wr[s.alp_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_wr -= 1;
                    if s.alp_wr < 0 || s.alp_wr > s.delay_w {
                        s.alp_wr = s.delay_w;
                    }
                    input_sample_r += s.a_wr[s.alp_wr as usize];
                    s.d_wr[2] = s.d_wr[1];
                    s.d_wr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.a_xr[allpasstemp as usize] * constallpass;
                    s.a_xr[s.alp_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_xr -= 1;
                    if s.alp_xr < 0 || s.alp_xr > s.delay_x {
                        s.alp_xr = s.delay_x;
                    }
                    input_sample_r += s.a_xr[s.alp_xr as usize];
                    s.d_xr[2] = s.d_xr[1];
                    s.d_xr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.a_yr[allpasstemp as usize] * constallpass;
                    s.a_yr[s.alp_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_yr -= 1;
                    if s.alp_yr < 0 || s.alp_yr > s.delay_y {
                        s.alp_yr = s.delay_y;
                    }
                    input_sample_r += s.a_yr[s.alp_yr as usize];
                    s.d_yr[2] = s.d_yr[1];
                    s.d_yr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.alp_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.a_zr[allpasstemp as usize] * constallpass;
                    s.a_zr[s.alp_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_zr -= 1;
                    if s.alp_zr < 0 || s.alp_zr > s.delay_z {
                        s.alp_zr = s.delay_z;
                    }
                    input_sample_r += s.a_zr[s.alp_zr as usize];
                    s.d_zr[2] = s.d_zr[1];
                    s.d_zr[1] = input_sample_r;
                    input_sample_r = dry_sample_r;
                    allpasstemp = s.out_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.o_ar[allpasstemp as usize] * constallpass;
                    s.o_ar[s.out_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ar -= 1;
                    if s.out_ar < 0 || s.out_ar > s.delay_a {
                        s.out_ar = s.delay_a;
                    }
                    input_sample_r += s.o_ar[s.out_ar as usize];
                    s.d_ar[5] = s.d_ar[4];
                    s.d_ar[4] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_ar[2]) / 2.0;
                    allpasstemp = s.out_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.o_br[allpasstemp as usize] * constallpass;
                    s.o_br[s.out_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_br -= 1;
                    if s.out_br < 0 || s.out_br > s.delay_b {
                        s.out_br = s.delay_b;
                    }
                    input_sample_r += s.o_br[s.out_br as usize];
                    s.d_br[5] = s.d_br[4];
                    s.d_br[4] = input_sample_r;
                    input_sample_r = (s.d_br[1] + s.d_br[2]) / 2.0;
                    allpasstemp = s.out_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.o_cr[allpasstemp as usize] * constallpass;
                    s.o_cr[s.out_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_cr -= 1;
                    if s.out_cr < 0 || s.out_cr > s.delay_c {
                        s.out_cr = s.delay_c;
                    }
                    input_sample_r += s.o_cr[s.out_cr as usize];
                    s.d_cr[5] = s.d_cr[4];
                    s.d_cr[4] = input_sample_r;
                    input_sample_r = (s.d_cr[1] + s.d_cr[2]) / 2.0;
                    allpasstemp = s.out_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.o_dr[allpasstemp as usize] * constallpass;
                    s.o_dr[s.out_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_dr -= 1;
                    if s.out_dr < 0 || s.out_dr > s.delay_d {
                        s.out_dr = s.delay_d;
                    }
                    input_sample_r += s.o_dr[s.out_dr as usize];
                    s.d_dr[5] = s.d_dr[4];
                    s.d_dr[4] = input_sample_r;
                    input_sample_r = (s.d_dr[1] + s.d_dr[2]) / 2.0;
                    allpasstemp = s.out_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.o_er[allpasstemp as usize] * constallpass;
                    s.o_er[s.out_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_er -= 1;
                    if s.out_er < 0 || s.out_er > s.delay_e {
                        s.out_er = s.delay_e;
                    }
                    input_sample_r += s.o_er[s.out_er as usize];
                    s.d_er[5] = s.d_er[4];
                    s.d_er[4] = input_sample_r;
                    input_sample_r = (s.d_er[1] + s.d_er[2]) / 2.0;
                    allpasstemp = s.out_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.o_fr[allpasstemp as usize] * constallpass;
                    s.o_fr[s.out_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_fr -= 1;
                    if s.out_fr < 0 || s.out_fr > s.delay_f {
                        s.out_fr = s.delay_f;
                    }
                    input_sample_r += s.o_fr[s.out_fr as usize];
                    s.d_fr[5] = s.d_fr[4];
                    s.d_fr[4] = input_sample_r;
                    input_sample_r = (s.d_fr[1] + s.d_fr[2]) / 2.0;
                    allpasstemp = s.out_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.o_gr[allpasstemp as usize] * constallpass;
                    s.o_gr[s.out_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_gr -= 1;
                    if s.out_gr < 0 || s.out_gr > s.delay_g {
                        s.out_gr = s.delay_g;
                    }
                    input_sample_r += s.o_gr[s.out_gr as usize];
                    s.d_gr[5] = s.d_gr[4];
                    s.d_gr[4] = input_sample_r;
                    input_sample_r = (s.d_gr[1] + s.d_gr[2]) / 2.0;
                    allpasstemp = s.out_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.o_hr[allpasstemp as usize] * constallpass;
                    s.o_hr[s.out_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_hr -= 1;
                    if s.out_hr < 0 || s.out_hr > s.delay_h {
                        s.out_hr = s.delay_h;
                    }
                    input_sample_r += s.o_hr[s.out_hr as usize];
                    s.d_hr[5] = s.d_hr[4];
                    s.d_hr[4] = input_sample_r;
                    input_sample_r = (s.d_hr[1] + s.d_hr[2]) / 2.0;
                    allpasstemp = s.out_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.o_ir[allpasstemp as usize] * constallpass;
                    s.o_ir[s.out_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ir -= 1;
                    if s.out_ir < 0 || s.out_ir > s.delay_i {
                        s.out_ir = s.delay_i;
                    }
                    input_sample_r += s.o_ir[s.out_ir as usize];
                    s.d_ir[5] = s.d_ir[4];
                    s.d_ir[4] = input_sample_r;
                    input_sample_r = (s.d_ir[1] + s.d_ir[2]) / 2.0;
                    allpasstemp = s.out_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.o_jr[allpasstemp as usize] * constallpass;
                    s.o_jr[s.out_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_jr -= 1;
                    if s.out_jr < 0 || s.out_jr > s.delay_j {
                        s.out_jr = s.delay_j;
                    }
                    input_sample_r += s.o_jr[s.out_jr as usize];
                    s.d_jr[5] = s.d_jr[4];
                    s.d_jr[4] = input_sample_r;
                    input_sample_r = (s.d_jr[1] + s.d_jr[2]) / 2.0;
                    allpasstemp = s.out_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.o_kr[allpasstemp as usize] * constallpass;
                    s.o_kr[s.out_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_kr -= 1;
                    if s.out_kr < 0 || s.out_kr > s.delay_k {
                        s.out_kr = s.delay_k;
                    }
                    input_sample_r += s.o_kr[s.out_kr as usize];
                    s.d_kr[5] = s.d_kr[4];
                    s.d_kr[4] = input_sample_r;
                    input_sample_r = (s.d_kr[1] + s.d_kr[2]) / 2.0;
                    allpasstemp = s.out_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.o_lr[allpasstemp as usize] * constallpass;
                    s.o_lr[s.out_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_lr -= 1;
                    if s.out_lr < 0 || s.out_lr > s.delay_l {
                        s.out_lr = s.delay_l;
                    }
                    input_sample_r += s.o_lr[s.out_lr as usize];
                    s.d_lr[5] = s.d_lr[4];
                    s.d_lr[4] = input_sample_r;
                    input_sample_r = (s.d_lr[1] + s.d_lr[2]) / 2.0;
                    allpasstemp = s.out_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.o_mr[allpasstemp as usize] * constallpass;
                    s.o_mr[s.out_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_mr -= 1;
                    if s.out_mr < 0 || s.out_mr > s.delay_m {
                        s.out_mr = s.delay_m;
                    }
                    input_sample_r += s.o_mr[s.out_mr as usize];
                    s.d_mr[5] = s.d_mr[4];
                    s.d_mr[4] = input_sample_r;
                    input_sample_r = (s.d_mr[1] + s.d_mr[2]) / 2.0;
                    allpasstemp = s.out_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.o_nr[allpasstemp as usize] * constallpass;
                    s.o_nr[s.out_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_nr -= 1;
                    if s.out_nr < 0 || s.out_nr > s.delay_n {
                        s.out_nr = s.delay_n;
                    }
                    input_sample_r += s.o_nr[s.out_nr as usize];
                    s.d_nr[5] = s.d_nr[4];
                    s.d_nr[4] = input_sample_r;
                    input_sample_r = (s.d_nr[1] + s.d_nr[2]) / 2.0;
                    allpasstemp = s.out_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.o_or[allpasstemp as usize] * constallpass;
                    s.o_or[s.out_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_or -= 1;
                    if s.out_or < 0 || s.out_or > s.delay_o {
                        s.out_or = s.delay_o;
                    }
                    input_sample_r += s.o_or[s.out_or as usize];
                    s.d_or[5] = s.d_or[4];
                    s.d_or[4] = input_sample_r;
                    input_sample_r = (s.d_or[1] + s.d_or[2]) / 2.0;
                    allpasstemp = s.out_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.o_pr[allpasstemp as usize] * constallpass;
                    s.o_pr[s.out_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_pr -= 1;
                    if s.out_pr < 0 || s.out_pr > s.delay_p {
                        s.out_pr = s.delay_p;
                    }
                    input_sample_r += s.o_pr[s.out_pr as usize];
                    s.d_pr[5] = s.d_pr[4];
                    s.d_pr[4] = input_sample_r;
                    input_sample_r = (s.d_pr[1] + s.d_pr[2]) / 2.0;
                    allpasstemp = s.out_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.o_qr[allpasstemp as usize] * constallpass;
                    s.o_qr[s.out_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_qr -= 1;
                    if s.out_qr < 0 || s.out_qr > s.delay_q {
                        s.out_qr = s.delay_q;
                    }
                    input_sample_r += s.o_qr[s.out_qr as usize];
                    s.d_qr[5] = s.d_qr[4];
                    s.d_qr[4] = input_sample_r;
                    input_sample_r = (s.d_qr[1] + s.d_qr[2]) / 2.0;
                    allpasstemp = s.out_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.o_rr[allpasstemp as usize] * constallpass;
                    s.o_rr[s.out_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_rr -= 1;
                    if s.out_rr < 0 || s.out_rr > s.delay_r {
                        s.out_rr = s.delay_r;
                    }
                    input_sample_r += s.o_rr[s.out_rr as usize];
                    s.d_rr[5] = s.d_rr[4];
                    s.d_rr[4] = input_sample_r;
                    input_sample_r = (s.d_rr[1] + s.d_rr[2]) / 2.0;
                    allpasstemp = s.out_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.o_sr[allpasstemp as usize] * constallpass;
                    s.o_sr[s.out_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_sr -= 1;
                    if s.out_sr < 0 || s.out_sr > s.delay_s {
                        s.out_sr = s.delay_s;
                    }
                    input_sample_r += s.o_sr[s.out_sr as usize];
                    s.d_sr[5] = s.d_sr[4];
                    s.d_sr[4] = input_sample_r;
                    input_sample_r = (s.d_sr[1] + s.d_sr[2]) / 2.0;
                    allpasstemp = s.out_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.o_tr[allpasstemp as usize] * constallpass;
                    s.o_tr[s.out_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_tr -= 1;
                    if s.out_tr < 0 || s.out_tr > s.delay_t {
                        s.out_tr = s.delay_t;
                    }
                    input_sample_r += s.o_tr[s.out_tr as usize];
                    s.d_tr[5] = s.d_tr[4];
                    s.d_tr[4] = input_sample_r;
                    input_sample_r = (s.d_tr[1] + s.d_tr[2]) / 2.0;
                    allpasstemp = s.out_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.o_ur[allpasstemp as usize] * constallpass;
                    s.o_ur[s.out_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ur -= 1;
                    if s.out_ur < 0 || s.out_ur > s.delay_u {
                        s.out_ur = s.delay_u;
                    }
                    input_sample_r += s.o_ur[s.out_ur as usize];
                    s.d_ur[5] = s.d_ur[4];
                    s.d_ur[4] = input_sample_r;
                    input_sample_r = (s.d_ur[1] + s.d_ur[2]) / 2.0;
                    allpasstemp = s.out_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.o_vr[allpasstemp as usize] * constallpass;
                    s.o_vr[s.out_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_vr -= 1;
                    if s.out_vr < 0 || s.out_vr > s.delay_v {
                        s.out_vr = s.delay_v;
                    }
                    input_sample_r += s.o_vr[s.out_vr as usize];
                    s.d_vr[5] = s.d_vr[4];
                    s.d_vr[4] = input_sample_r;
                    input_sample_r = (s.d_vr[1] + s.d_vr[2]) / 2.0;
                    allpasstemp = s.out_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.o_wr[allpasstemp as usize] * constallpass;
                    s.o_wr[s.out_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_wr -= 1;
                    if s.out_wr < 0 || s.out_wr > s.delay_w {
                        s.out_wr = s.delay_w;
                    }
                    input_sample_r += s.o_wr[s.out_wr as usize];
                    s.d_wr[5] = s.d_wr[4];
                    s.d_wr[4] = input_sample_r;
                    input_sample_r = (s.d_wr[1] + s.d_wr[2]) / 2.0;
                    allpasstemp = s.out_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.o_xr[allpasstemp as usize] * constallpass;
                    s.o_xr[s.out_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_xr -= 1;
                    if s.out_xr < 0 || s.out_xr > s.delay_x {
                        s.out_xr = s.delay_x;
                    }
                    input_sample_r += s.o_xr[s.out_xr as usize];
                    s.d_xr[5] = s.d_xr[4];
                    s.d_xr[4] = input_sample_r;
                    input_sample_r = (s.d_xr[1] + s.d_xr[2]) / 2.0;
                    allpasstemp = s.out_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.o_yr[allpasstemp as usize] * constallpass;
                    s.o_yr[s.out_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_yr -= 1;
                    if s.out_yr < 0 || s.out_yr > s.delay_y {
                        s.out_yr = s.delay_y;
                    }
                    input_sample_r += s.o_yr[s.out_yr as usize];
                    s.d_yr[5] = s.d_yr[4];
                    s.d_yr[4] = input_sample_r;
                    input_sample_r = (s.d_yr[1] + s.d_yr[2]) / 2.0;
                    allpasstemp = s.out_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.o_zr[allpasstemp as usize] * constallpass;
                    s.o_zr[s.out_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_zr -= 1;
                    if s.out_zr < 0 || s.out_zr > s.delay_z {
                        s.out_zr = s.delay_z;
                    }
                    input_sample_r += s.o_zr[s.out_zr as usize];
                    s.d_zr[5] = s.d_zr[4];
                    s.d_zr[4] = input_sample_r;
                    input_sample_r = s.d_br[4] * dryness;
                    input_sample_r += s.d_cr[4] * dryness;
                    input_sample_r += s.d_dr[4];
                    input_sample_r += s.d_er[4];
                    input_sample_r += s.d_fr[4];
                    input_sample_r += s.d_gr[4];
                    input_sample_r += s.d_hr[4];
                    input_sample_r += s.d_ir[4];
                    input_sample_r += s.d_jr[4];
                    input_sample_r += s.d_kr[4];
                    input_sample_r += s.d_lr[4];
                    input_sample_r += s.d_mr[4];
                    input_sample_r += s.d_nr[4];
                    input_sample_r += s.d_or[4];
                    input_sample_r += s.d_pr[4];
                    input_sample_r += s.d_qr[4];
                    input_sample_r += s.d_rr[4];
                    input_sample_r += s.d_sr[4];
                    input_sample_r += s.d_tr[4];
                    input_sample_r += s.d_ur[4];
                    input_sample_r += s.d_vr[4];
                    input_sample_r += s.d_wr[4];
                    input_sample_r += s.d_xr[4];
                    input_sample_r += s.d_yr[4];
                    input_sample_r += s.d_zr[4] * wetness;
                    input_sample_r += s.d_br[5] * dryness;
                    input_sample_r += s.d_cr[5] * dryness;
                    input_sample_r += s.d_dr[5];
                    input_sample_r += s.d_er[5];
                    input_sample_r += s.d_fr[5];
                    input_sample_r += s.d_gr[5];
                    input_sample_r += s.d_hr[5];
                    input_sample_r += s.d_ir[5];
                    input_sample_r += s.d_jr[5];
                    input_sample_r += s.d_kr[5];
                    input_sample_r += s.d_lr[5];
                    input_sample_r += s.d_mr[5];
                    input_sample_r += s.d_nr[5];
                    input_sample_r += s.d_or[5];
                    input_sample_r += s.d_pr[5];
                    input_sample_r += s.d_qr[5];
                    input_sample_r += s.d_rr[5];
                    input_sample_r += s.d_sr[5];
                    input_sample_r += s.d_tr[5];
                    input_sample_r += s.d_ur[5];
                    input_sample_r += s.d_vr[5];
                    input_sample_r += s.d_wr[5];
                    input_sample_r += s.d_xr[5];
                    input_sample_r += s.d_yr[5];
                    input_sample_r += s.d_zr[5] * wetness;
                    input_sample_r /= 26.0 + wetness * 4.0;
                }
                5 => {
                    allpasstemp = s.alp_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.a_ar[allpasstemp as usize] * constallpass;
                    s.a_ar[s.alp_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ar -= 1;
                    if s.alp_ar < 0 || s.alp_ar > s.delay_a {
                        s.alp_ar = s.delay_a;
                    }
                    input_sample_r += s.a_ar[s.alp_ar as usize];
                    s.d_ar[2] = s.d_ar[1];
                    s.d_ar[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_ar[2]) / 2.0;
                    allpasstemp = s.alp_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.a_br[allpasstemp as usize] * constallpass;
                    s.a_br[s.alp_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_br -= 1;
                    if s.alp_br < 0 || s.alp_br > s.delay_b {
                        s.alp_br = s.delay_b;
                    }
                    input_sample_r += s.a_br[s.alp_br as usize];
                    s.d_br[2] = s.d_br[1];
                    s.d_br[1] = input_sample_r;
                    input_sample_r = (s.d_br[1] + s.d_br[2]) / 2.0;
                    allpasstemp = s.alp_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.a_cr[allpasstemp as usize] * constallpass;
                    s.a_cr[s.alp_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_cr -= 1;
                    if s.alp_cr < 0 || s.alp_cr > s.delay_c {
                        s.alp_cr = s.delay_c;
                    }
                    input_sample_r += s.a_cr[s.alp_cr as usize];
                    s.d_cr[2] = s.d_cr[1];
                    s.d_cr[1] = input_sample_r;
                    input_sample_r = (s.d_cr[1] + s.d_cr[2]) / 2.0;
                    allpasstemp = s.alp_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.a_dr[allpasstemp as usize] * constallpass;
                    s.a_dr[s.alp_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_dr -= 1;
                    if s.alp_dr < 0 || s.alp_dr > s.delay_d {
                        s.alp_dr = s.delay_d;
                    }
                    input_sample_r += s.a_dr[s.alp_dr as usize];
                    s.d_dr[2] = s.d_dr[1];
                    s.d_dr[1] = input_sample_r;
                    input_sample_r = (s.d_dr[1] + s.d_dr[2]) / 2.0;
                    allpasstemp = s.alp_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.a_er[allpasstemp as usize] * constallpass;
                    s.a_er[s.alp_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_er -= 1;
                    if s.alp_er < 0 || s.alp_er > s.delay_e {
                        s.alp_er = s.delay_e;
                    }
                    input_sample_r += s.a_er[s.alp_er as usize];
                    s.d_er[2] = s.d_er[1];
                    s.d_er[1] = input_sample_r;
                    input_sample_r = (s.d_er[1] + s.d_er[2]) / 2.0;
                    allpasstemp = s.alp_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.a_fr[allpasstemp as usize] * constallpass;
                    s.a_fr[s.alp_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_fr -= 1;
                    if s.alp_fr < 0 || s.alp_fr > s.delay_f {
                        s.alp_fr = s.delay_f;
                    }
                    input_sample_r += s.a_fr[s.alp_fr as usize];
                    s.d_fr[2] = s.d_fr[1];
                    s.d_fr[1] = input_sample_r;
                    input_sample_r = (s.d_fr[1] + s.d_fr[2]) / 2.0;
                    allpasstemp = s.alp_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.a_gr[allpasstemp as usize] * constallpass;
                    s.a_gr[s.alp_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_gr -= 1;
                    if s.alp_gr < 0 || s.alp_gr > s.delay_g {
                        s.alp_gr = s.delay_g;
                    }
                    input_sample_r += s.a_gr[s.alp_gr as usize];
                    s.d_gr[2] = s.d_gr[1];
                    s.d_gr[1] = input_sample_r;
                    input_sample_r = (s.d_gr[1] + s.d_gr[2]) / 2.0;
                    allpasstemp = s.alp_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.a_hr[allpasstemp as usize] * constallpass;
                    s.a_hr[s.alp_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_hr -= 1;
                    if s.alp_hr < 0 || s.alp_hr > s.delay_h {
                        s.alp_hr = s.delay_h;
                    }
                    input_sample_r += s.a_hr[s.alp_hr as usize];
                    s.d_hr[2] = s.d_hr[1];
                    s.d_hr[1] = input_sample_r;
                    input_sample_r = (s.d_hr[1] + s.d_hr[2]) / 2.0;
                    allpasstemp = s.alp_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.a_ir[allpasstemp as usize] * constallpass;
                    s.a_ir[s.alp_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ir -= 1;
                    if s.alp_ir < 0 || s.alp_ir > s.delay_i {
                        s.alp_ir = s.delay_i;
                    }
                    input_sample_r += s.a_ir[s.alp_ir as usize];
                    s.d_ir[2] = s.d_ir[1];
                    s.d_ir[1] = input_sample_r;
                    input_sample_r = (s.d_ir[1] + s.d_ir[2]) / 2.0;
                    allpasstemp = s.alp_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.a_jr[allpasstemp as usize] * constallpass;
                    s.a_jr[s.alp_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_jr -= 1;
                    if s.alp_jr < 0 || s.alp_jr > s.delay_j {
                        s.alp_jr = s.delay_j;
                    }
                    input_sample_r += s.a_jr[s.alp_jr as usize];
                    s.d_jr[2] = s.d_jr[1];
                    s.d_jr[1] = input_sample_r;
                    input_sample_r = (s.d_jr[1] + s.d_jr[2]) / 2.0;
                    allpasstemp = s.alp_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.a_kr[allpasstemp as usize] * constallpass;
                    s.a_kr[s.alp_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_kr -= 1;
                    if s.alp_kr < 0 || s.alp_kr > s.delay_k {
                        s.alp_kr = s.delay_k;
                    }
                    input_sample_r += s.a_kr[s.alp_kr as usize];
                    s.d_kr[2] = s.d_kr[1];
                    s.d_kr[1] = input_sample_r;
                    input_sample_r = (s.d_kr[1] + s.d_kr[2]) / 2.0;
                    allpasstemp = s.alp_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.a_lr[allpasstemp as usize] * constallpass;
                    s.a_lr[s.alp_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_lr -= 1;
                    if s.alp_lr < 0 || s.alp_lr > s.delay_l {
                        s.alp_lr = s.delay_l;
                    }
                    input_sample_r += s.a_lr[s.alp_lr as usize];
                    s.d_lr[2] = s.d_lr[1];
                    s.d_lr[1] = input_sample_r;
                    input_sample_r = (s.d_lr[1] + s.d_lr[2]) / 2.0;
                    allpasstemp = s.alp_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.a_mr[allpasstemp as usize] * constallpass;
                    s.a_mr[s.alp_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_mr -= 1;
                    if s.alp_mr < 0 || s.alp_mr > s.delay_m {
                        s.alp_mr = s.delay_m;
                    }
                    input_sample_r += s.a_mr[s.alp_mr as usize];
                    s.d_mr[2] = s.d_mr[1];
                    s.d_mr[1] = input_sample_r;
                    input_sample_r = (s.d_mr[1] + s.d_mr[2]) / 2.0;
                    allpasstemp = s.alp_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.a_nr[allpasstemp as usize] * constallpass;
                    s.a_nr[s.alp_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_nr -= 1;
                    if s.alp_nr < 0 || s.alp_nr > s.delay_n {
                        s.alp_nr = s.delay_n;
                    }
                    input_sample_r += s.a_nr[s.alp_nr as usize];
                    s.d_nr[2] = s.d_nr[1];
                    s.d_nr[1] = input_sample_r;
                    input_sample_r = (s.d_nr[1] + s.d_nr[2]) / 2.0;
                    allpasstemp = s.alp_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.a_or[allpasstemp as usize] * constallpass;
                    s.a_or[s.alp_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_or -= 1;
                    if s.alp_or < 0 || s.alp_or > s.delay_o {
                        s.alp_or = s.delay_o;
                    }
                    input_sample_r += s.a_or[s.alp_or as usize];
                    s.d_or[2] = s.d_or[1];
                    s.d_or[1] = input_sample_r;
                    input_sample_r = (s.d_or[1] + s.d_or[2]) / 2.0;
                    allpasstemp = s.alp_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.a_pr[allpasstemp as usize] * constallpass;
                    s.a_pr[s.alp_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_pr -= 1;
                    if s.alp_pr < 0 || s.alp_pr > s.delay_p {
                        s.alp_pr = s.delay_p;
                    }
                    input_sample_r += s.a_pr[s.alp_pr as usize];
                    s.d_pr[2] = s.d_pr[1];
                    s.d_pr[1] = input_sample_r;
                    input_sample_r = (s.d_pr[1] + s.d_pr[2]) / 2.0;
                    allpasstemp = s.alp_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.a_qr[allpasstemp as usize] * constallpass;
                    s.a_qr[s.alp_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_qr -= 1;
                    if s.alp_qr < 0 || s.alp_qr > s.delay_q {
                        s.alp_qr = s.delay_q;
                    }
                    input_sample_r += s.a_qr[s.alp_qr as usize];
                    s.d_qr[2] = s.d_qr[1];
                    s.d_qr[1] = input_sample_r;
                    input_sample_r = (s.d_qr[1] + s.d_qr[2]) / 2.0;
                    allpasstemp = s.alp_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.a_rr[allpasstemp as usize] * constallpass;
                    s.a_rr[s.alp_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_rr -= 1;
                    if s.alp_rr < 0 || s.alp_rr > s.delay_r {
                        s.alp_rr = s.delay_r;
                    }
                    input_sample_r += s.a_rr[s.alp_rr as usize];
                    s.d_rr[2] = s.d_rr[1];
                    s.d_rr[1] = input_sample_r;
                    input_sample_r = (s.d_rr[1] + s.d_rr[2]) / 2.0;
                    allpasstemp = s.alp_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.a_sr[allpasstemp as usize] * constallpass;
                    s.a_sr[s.alp_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_sr -= 1;
                    if s.alp_sr < 0 || s.alp_sr > s.delay_s {
                        s.alp_sr = s.delay_s;
                    }
                    input_sample_r += s.a_sr[s.alp_sr as usize];
                    s.d_sr[2] = s.d_sr[1];
                    s.d_sr[1] = input_sample_r;
                    input_sample_r = (s.d_sr[1] + s.d_sr[2]) / 2.0;
                    allpasstemp = s.alp_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.a_tr[allpasstemp as usize] * constallpass;
                    s.a_tr[s.alp_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_tr -= 1;
                    if s.alp_tr < 0 || s.alp_tr > s.delay_t {
                        s.alp_tr = s.delay_t;
                    }
                    input_sample_r += s.a_tr[s.alp_tr as usize];
                    s.d_tr[2] = s.d_tr[1];
                    s.d_tr[1] = input_sample_r;
                    input_sample_r = (s.d_tr[1] + s.d_tr[2]) / 2.0;
                    allpasstemp = s.alp_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.a_ur[allpasstemp as usize] * constallpass;
                    s.a_ur[s.alp_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ur -= 1;
                    if s.alp_ur < 0 || s.alp_ur > s.delay_u {
                        s.alp_ur = s.delay_u;
                    }
                    input_sample_r += s.a_ur[s.alp_ur as usize];
                    s.d_ur[2] = s.d_ur[1];
                    s.d_ur[1] = input_sample_r;
                    input_sample_r = (s.d_ur[1] + s.d_ur[2]) / 2.0;
                    allpasstemp = s.alp_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.a_vr[allpasstemp as usize] * constallpass;
                    s.a_vr[s.alp_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_vr -= 1;
                    if s.alp_vr < 0 || s.alp_vr > s.delay_v {
                        s.alp_vr = s.delay_v;
                    }
                    input_sample_r += s.a_vr[s.alp_vr as usize];
                    s.d_vr[2] = s.d_vr[1];
                    s.d_vr[1] = input_sample_r;
                    input_sample_r = (s.d_vr[1] + s.d_vr[2]) / 2.0;
                    allpasstemp = s.alp_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.a_wr[allpasstemp as usize] * constallpass;
                    s.a_wr[s.alp_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_wr -= 1;
                    if s.alp_wr < 0 || s.alp_wr > s.delay_w {
                        s.alp_wr = s.delay_w;
                    }
                    input_sample_r += s.a_wr[s.alp_wr as usize];
                    s.d_wr[2] = s.d_wr[1];
                    s.d_wr[1] = input_sample_r;
                    input_sample_r = (s.d_wr[1] + s.d_wr[2]) / 2.0;
                    allpasstemp = s.alp_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.a_xr[allpasstemp as usize] * constallpass;
                    s.a_xr[s.alp_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_xr -= 1;
                    if s.alp_xr < 0 || s.alp_xr > s.delay_x {
                        s.alp_xr = s.delay_x;
                    }
                    input_sample_r += s.a_xr[s.alp_xr as usize];
                    s.d_xr[2] = s.d_xr[1];
                    s.d_xr[1] = input_sample_r;
                    input_sample_r = (s.d_xr[1] + s.d_xr[2]) / 2.0;
                    allpasstemp = s.alp_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.a_yr[allpasstemp as usize] * constallpass;
                    s.a_yr[s.alp_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_yr -= 1;
                    if s.alp_yr < 0 || s.alp_yr > s.delay_y {
                        s.alp_yr = s.delay_y;
                    }
                    input_sample_r += s.a_yr[s.alp_yr as usize];
                    s.d_yr[2] = s.d_yr[1];
                    s.d_yr[1] = input_sample_r;
                    input_sample_r = (s.d_yr[1] + s.d_yr[2]) / 2.0;
                    allpasstemp = s.alp_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.a_zr[allpasstemp as usize] * constallpass;
                    s.a_zr[s.alp_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_zr -= 1;
                    if s.alp_zr < 0 || s.alp_zr > s.delay_z {
                        s.alp_zr = s.delay_z;
                    }
                    input_sample_r += s.a_zr[s.alp_zr as usize];
                    s.d_zr[2] = s.d_zr[1];
                    s.d_zr[1] = input_sample_r;
                    input_sample_r = (s.d_zr[1] + s.d_zr[2]) / 2.0;
                    allpasstemp = s.out_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.o_ar[allpasstemp as usize] * constallpass;
                    s.o_ar[s.out_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ar -= 1;
                    if s.out_ar < 0 || s.out_ar > s.delay_a {
                        s.out_ar = s.delay_a;
                    }
                    input_sample_r += s.o_ar[s.out_ar as usize];
                    s.d_ar[5] = s.d_ar[4];
                    s.d_ar[4] = input_sample_r;
                    input_sample_r = (s.d_ar[4] + s.d_ar[5]) / 2.0;
                    allpasstemp = s.out_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.o_br[allpasstemp as usize] * constallpass;
                    s.o_br[s.out_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_br -= 1;
                    if s.out_br < 0 || s.out_br > s.delay_b {
                        s.out_br = s.delay_b;
                    }
                    input_sample_r += s.o_br[s.out_br as usize];
                    s.d_br[5] = s.d_br[4];
                    s.d_br[4] = input_sample_r;
                    input_sample_r = (s.d_br[4] + s.d_br[5]) / 2.0;
                    allpasstemp = s.out_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.o_cr[allpasstemp as usize] * constallpass;
                    s.o_cr[s.out_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_cr -= 1;
                    if s.out_cr < 0 || s.out_cr > s.delay_c {
                        s.out_cr = s.delay_c;
                    }
                    input_sample_r += s.o_cr[s.out_cr as usize];
                    s.d_cr[5] = s.d_cr[4];
                    s.d_cr[4] = input_sample_r;
                    input_sample_r = (s.d_cr[4] + s.d_cr[5]) / 2.0;
                    allpasstemp = s.out_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.o_dr[allpasstemp as usize] * constallpass;
                    s.o_dr[s.out_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_dr -= 1;
                    if s.out_dr < 0 || s.out_dr > s.delay_d {
                        s.out_dr = s.delay_d;
                    }
                    input_sample_r += s.o_dr[s.out_dr as usize];
                    s.d_dr[5] = s.d_dr[4];
                    s.d_dr[4] = input_sample_r;
                    input_sample_r = (s.d_dr[4] + s.d_dr[5]) / 2.0;
                    allpasstemp = s.out_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.o_er[allpasstemp as usize] * constallpass;
                    s.o_er[s.out_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_er -= 1;
                    if s.out_er < 0 || s.out_er > s.delay_e {
                        s.out_er = s.delay_e;
                    }
                    input_sample_r += s.o_er[s.out_er as usize];
                    s.d_er[5] = s.d_er[4];
                    s.d_er[4] = input_sample_r;
                    input_sample_r = (s.d_er[4] + s.d_er[5]) / 2.0;
                    allpasstemp = s.out_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.o_fr[allpasstemp as usize] * constallpass;
                    s.o_fr[s.out_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_fr -= 1;
                    if s.out_fr < 0 || s.out_fr > s.delay_f {
                        s.out_fr = s.delay_f;
                    }
                    input_sample_r += s.o_fr[s.out_fr as usize];
                    s.d_fr[5] = s.d_fr[4];
                    s.d_fr[4] = input_sample_r;
                    input_sample_r = (s.d_fr[4] + s.d_fr[5]) / 2.0;
                    allpasstemp = s.out_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.o_gr[allpasstemp as usize] * constallpass;
                    s.o_gr[s.out_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_gr -= 1;
                    if s.out_gr < 0 || s.out_gr > s.delay_g {
                        s.out_gr = s.delay_g;
                    }
                    input_sample_r += s.o_gr[s.out_gr as usize];
                    s.d_gr[5] = s.d_gr[4];
                    s.d_gr[4] = input_sample_r;
                    input_sample_r = (s.d_gr[4] + s.d_gr[5]) / 2.0;
                    allpasstemp = s.out_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.o_hr[allpasstemp as usize] * constallpass;
                    s.o_hr[s.out_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_hr -= 1;
                    if s.out_hr < 0 || s.out_hr > s.delay_h {
                        s.out_hr = s.delay_h;
                    }
                    input_sample_r += s.o_hr[s.out_hr as usize];
                    s.d_hr[5] = s.d_hr[4];
                    s.d_hr[4] = input_sample_r;
                    input_sample_r = (s.d_hr[4] + s.d_hr[5]) / 2.0;
                    allpasstemp = s.out_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.o_ir[allpasstemp as usize] * constallpass;
                    s.o_ir[s.out_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ir -= 1;
                    if s.out_ir < 0 || s.out_ir > s.delay_i {
                        s.out_ir = s.delay_i;
                    }
                    input_sample_r += s.o_ir[s.out_ir as usize];
                    s.d_ir[5] = s.d_ir[4];
                    s.d_ir[4] = input_sample_r;
                    input_sample_r = (s.d_ir[4] + s.d_ir[5]) / 2.0;
                    allpasstemp = s.out_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.o_jr[allpasstemp as usize] * constallpass;
                    s.o_jr[s.out_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_jr -= 1;
                    if s.out_jr < 0 || s.out_jr > s.delay_j {
                        s.out_jr = s.delay_j;
                    }
                    input_sample_r += s.o_jr[s.out_jr as usize];
                    s.d_jr[5] = s.d_jr[4];
                    s.d_jr[4] = input_sample_r;
                    input_sample_r = (s.d_jr[4] + s.d_jr[5]) / 2.0;
                    allpasstemp = s.out_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.o_kr[allpasstemp as usize] * constallpass;
                    s.o_kr[s.out_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_kr -= 1;
                    if s.out_kr < 0 || s.out_kr > s.delay_k {
                        s.out_kr = s.delay_k;
                    }
                    input_sample_r += s.o_kr[s.out_kr as usize];
                    s.d_kr[5] = s.d_kr[4];
                    s.d_kr[4] = input_sample_r;
                    input_sample_r = (s.d_kr[4] + s.d_kr[5]) / 2.0;
                    allpasstemp = s.out_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.o_lr[allpasstemp as usize] * constallpass;
                    s.o_lr[s.out_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_lr -= 1;
                    if s.out_lr < 0 || s.out_lr > s.delay_l {
                        s.out_lr = s.delay_l;
                    }
                    input_sample_r += s.o_lr[s.out_lr as usize];
                    s.d_lr[5] = s.d_lr[4];
                    s.d_lr[4] = input_sample_r;
                    input_sample_r = (s.d_lr[4] + s.d_lr[5]) / 2.0;
                    allpasstemp = s.out_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.o_mr[allpasstemp as usize] * constallpass;
                    s.o_mr[s.out_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_mr -= 1;
                    if s.out_mr < 0 || s.out_mr > s.delay_m {
                        s.out_mr = s.delay_m;
                    }
                    input_sample_r += s.o_mr[s.out_mr as usize];
                    s.d_mr[5] = s.d_mr[4];
                    s.d_mr[4] = input_sample_r;
                    input_sample_r = (s.d_mr[4] + s.d_mr[5]) / 2.0;
                    allpasstemp = s.out_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.o_nr[allpasstemp as usize] * constallpass;
                    s.o_nr[s.out_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_nr -= 1;
                    if s.out_nr < 0 || s.out_nr > s.delay_n {
                        s.out_nr = s.delay_n;
                    }
                    input_sample_r += s.o_nr[s.out_nr as usize];
                    s.d_nr[5] = s.d_nr[4];
                    s.d_nr[4] = input_sample_r;
                    input_sample_r = (s.d_nr[4] + s.d_nr[5]) / 2.0;
                    allpasstemp = s.out_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.o_or[allpasstemp as usize] * constallpass;
                    s.o_or[s.out_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_or -= 1;
                    if s.out_or < 0 || s.out_or > s.delay_o {
                        s.out_or = s.delay_o;
                    }
                    input_sample_r += s.o_or[s.out_or as usize];
                    s.d_or[5] = s.d_or[4];
                    s.d_or[4] = input_sample_r;
                    input_sample_r = (s.d_or[4] + s.d_or[5]) / 2.0;
                    allpasstemp = s.out_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.o_pr[allpasstemp as usize] * constallpass;
                    s.o_pr[s.out_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_pr -= 1;
                    if s.out_pr < 0 || s.out_pr > s.delay_p {
                        s.out_pr = s.delay_p;
                    }
                    input_sample_r += s.o_pr[s.out_pr as usize];
                    s.d_pr[5] = s.d_pr[4];
                    s.d_pr[4] = input_sample_r;
                    input_sample_r = (s.d_pr[4] + s.d_pr[5]) / 2.0;
                    allpasstemp = s.out_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.o_qr[allpasstemp as usize] * constallpass;
                    s.o_qr[s.out_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_qr -= 1;
                    if s.out_qr < 0 || s.out_qr > s.delay_q {
                        s.out_qr = s.delay_q;
                    }
                    input_sample_r += s.o_qr[s.out_qr as usize];
                    s.d_qr[5] = s.d_qr[4];
                    s.d_qr[4] = input_sample_r;
                    input_sample_r = (s.d_qr[4] + s.d_qr[5]) / 2.0;
                    allpasstemp = s.out_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.o_rr[allpasstemp as usize] * constallpass;
                    s.o_rr[s.out_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_rr -= 1;
                    if s.out_rr < 0 || s.out_rr > s.delay_r {
                        s.out_rr = s.delay_r;
                    }
                    input_sample_r += s.o_rr[s.out_rr as usize];
                    s.d_rr[5] = s.d_rr[4];
                    s.d_rr[4] = input_sample_r;
                    input_sample_r = (s.d_rr[4] + s.d_rr[5]) / 2.0;
                    allpasstemp = s.out_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.o_sr[allpasstemp as usize] * constallpass;
                    s.o_sr[s.out_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_sr -= 1;
                    if s.out_sr < 0 || s.out_sr > s.delay_s {
                        s.out_sr = s.delay_s;
                    }
                    input_sample_r += s.o_sr[s.out_sr as usize];
                    s.d_sr[5] = s.d_sr[4];
                    s.d_sr[4] = input_sample_r;
                    input_sample_r = (s.d_sr[4] + s.d_sr[5]) / 2.0;
                    allpasstemp = s.out_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.o_tr[allpasstemp as usize] * constallpass;
                    s.o_tr[s.out_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_tr -= 1;
                    if s.out_tr < 0 || s.out_tr > s.delay_t {
                        s.out_tr = s.delay_t;
                    }
                    input_sample_r += s.o_tr[s.out_tr as usize];
                    s.d_tr[5] = s.d_tr[4];
                    s.d_tr[4] = input_sample_r;
                    input_sample_r = (s.d_tr[4] + s.d_tr[5]) / 2.0;
                    allpasstemp = s.out_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.o_ur[allpasstemp as usize] * constallpass;
                    s.o_ur[s.out_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ur -= 1;
                    if s.out_ur < 0 || s.out_ur > s.delay_u {
                        s.out_ur = s.delay_u;
                    }
                    input_sample_r += s.o_ur[s.out_ur as usize];
                    s.d_ur[5] = s.d_ur[4];
                    s.d_ur[4] = input_sample_r;
                    input_sample_r = (s.d_ur[4] + s.d_ur[5]) / 2.0;
                    allpasstemp = s.out_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.o_vr[allpasstemp as usize] * constallpass;
                    s.o_vr[s.out_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_vr -= 1;
                    if s.out_vr < 0 || s.out_vr > s.delay_v {
                        s.out_vr = s.delay_v;
                    }
                    input_sample_r += s.o_vr[s.out_vr as usize];
                    s.d_vr[5] = s.d_vr[4];
                    s.d_vr[4] = input_sample_r;
                    input_sample_r = (s.d_vr[4] + s.d_vr[5]) / 2.0;
                    allpasstemp = s.out_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.o_wr[allpasstemp as usize] * constallpass;
                    s.o_wr[s.out_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_wr -= 1;
                    if s.out_wr < 0 || s.out_wr > s.delay_w {
                        s.out_wr = s.delay_w;
                    }
                    input_sample_r += s.o_wr[s.out_wr as usize];
                    s.d_wr[5] = s.d_wr[4];
                    s.d_wr[4] = input_sample_r;
                    input_sample_r = (s.d_wr[4] + s.d_wr[5]) / 2.0;
                    allpasstemp = s.out_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.o_xr[allpasstemp as usize] * constallpass;
                    s.o_xr[s.out_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_xr -= 1;
                    if s.out_xr < 0 || s.out_xr > s.delay_x {
                        s.out_xr = s.delay_x;
                    }
                    input_sample_r += s.o_xr[s.out_xr as usize];
                    s.d_xr[5] = s.d_xr[4];
                    s.d_xr[4] = input_sample_r;
                    input_sample_r = (s.d_xr[4] + s.d_xr[5]) / 2.0;
                    allpasstemp = s.out_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.o_yr[allpasstemp as usize] * constallpass;
                    s.o_yr[s.out_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_yr -= 1;
                    if s.out_yr < 0 || s.out_yr > s.delay_y {
                        s.out_yr = s.delay_y;
                    }
                    input_sample_r += s.o_yr[s.out_yr as usize];
                    s.d_yr[5] = s.d_yr[4];
                    s.d_yr[4] = input_sample_r;
                    input_sample_r = (s.d_yr[4] + s.d_yr[5]) / 2.0;
                    allpasstemp = s.out_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.o_zr[allpasstemp as usize] * constallpass;
                    s.o_zr[s.out_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_zr -= 1;
                    if s.out_zr < 0 || s.out_zr > s.delay_z {
                        s.out_zr = s.delay_z;
                    }
                    input_sample_r += s.o_zr[s.out_zr as usize];
                    s.d_zr[5] = s.d_zr[4];
                    s.d_zr[4] = input_sample_r;
                    input_sample_r = (s.d_zr[4] + s.d_zr[5]) / 2.0;
                }
                6 => {
                    allpasstemp = s.alp_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.a_ar[allpasstemp as usize] * constallpass;
                    s.a_ar[s.alp_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ar -= 1;
                    if s.alp_ar < 0 || s.alp_ar > s.delay_a {
                        s.alp_ar = s.delay_a;
                    }
                    input_sample_r += s.a_ar[s.alp_ar as usize];
                    s.d_ar[3] = s.d_ar[2];
                    s.d_ar[2] = s.d_ar[1];
                    s.d_ar[1] = input_sample_r;
                    input_sample_r = (s.d_ar[1] + s.d_ar[2] + s.d_zr[3]) / 3.0;
                    allpasstemp = s.alp_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.a_br[allpasstemp as usize] * constallpass;
                    s.a_br[s.alp_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_br -= 1;
                    if s.alp_br < 0 || s.alp_br > s.delay_b {
                        s.alp_br = s.delay_b;
                    }
                    input_sample_r += s.a_br[s.alp_br as usize];
                    s.d_br[3] = s.d_br[2];
                    s.d_br[2] = s.d_br[1];
                    s.d_br[1] = input_sample_r;
                    input_sample_r = (s.d_br[1] + s.d_br[2] + s.d_br[3]) / 3.0;
                    allpasstemp = s.alp_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.a_cr[allpasstemp as usize] * constallpass;
                    s.a_cr[s.alp_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_cr -= 1;
                    if s.alp_cr < 0 || s.alp_cr > s.delay_c {
                        s.alp_cr = s.delay_c;
                    }
                    input_sample_r += s.a_cr[s.alp_cr as usize];
                    s.d_cr[3] = s.d_cr[2];
                    s.d_cr[2] = s.d_cr[1];
                    s.d_cr[1] = input_sample_r;
                    input_sample_r = (s.d_cr[1] + s.d_cr[2] + s.d_cr[3]) / 3.0;
                    allpasstemp = s.alp_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.a_dr[allpasstemp as usize] * constallpass;
                    s.a_dr[s.alp_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_dr -= 1;
                    if s.alp_dr < 0 || s.alp_dr > s.delay_d {
                        s.alp_dr = s.delay_d;
                    }
                    input_sample_r += s.a_dr[s.alp_dr as usize];
                    s.d_dr[3] = s.d_dr[2];
                    s.d_dr[2] = s.d_dr[1];
                    s.d_dr[1] = input_sample_r;
                    input_sample_r = (s.d_dr[1] + s.d_dr[2] + s.d_dr[3]) / 3.0;
                    allpasstemp = s.alp_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.a_er[allpasstemp as usize] * constallpass;
                    s.a_er[s.alp_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_er -= 1;
                    if s.alp_er < 0 || s.alp_er > s.delay_e {
                        s.alp_er = s.delay_e;
                    }
                    input_sample_r += s.a_er[s.alp_er as usize];
                    s.d_er[3] = s.d_er[2];
                    s.d_er[2] = s.d_er[1];
                    s.d_er[1] = input_sample_r;
                    input_sample_r = (s.d_er[1] + s.d_er[2] + s.d_er[3]) / 3.0;
                    allpasstemp = s.alp_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.a_fr[allpasstemp as usize] * constallpass;
                    s.a_fr[s.alp_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_fr -= 1;
                    if s.alp_fr < 0 || s.alp_fr > s.delay_f {
                        s.alp_fr = s.delay_f;
                    }
                    input_sample_r += s.a_fr[s.alp_fr as usize];
                    s.d_fr[3] = s.d_fr[2];
                    s.d_fr[2] = s.d_fr[1];
                    s.d_fr[1] = input_sample_r;
                    input_sample_r = (s.d_fr[1] + s.d_fr[2] + s.d_fr[3]) / 3.0;
                    allpasstemp = s.alp_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.a_gr[allpasstemp as usize] * constallpass;
                    s.a_gr[s.alp_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_gr -= 1;
                    if s.alp_gr < 0 || s.alp_gr > s.delay_g {
                        s.alp_gr = s.delay_g;
                    }
                    input_sample_r += s.a_gr[s.alp_gr as usize];
                    s.d_gr[3] = s.d_gr[2];
                    s.d_gr[2] = s.d_gr[1];
                    s.d_gr[1] = input_sample_r;
                    input_sample_r = (s.d_gr[1] + s.d_gr[2] + s.d_gr[3]) / 3.0;
                    allpasstemp = s.alp_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.a_hr[allpasstemp as usize] * constallpass;
                    s.a_hr[s.alp_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_hr -= 1;
                    if s.alp_hr < 0 || s.alp_hr > s.delay_h {
                        s.alp_hr = s.delay_h;
                    }
                    input_sample_r += s.a_hr[s.alp_hr as usize];
                    s.d_hr[3] = s.d_hr[2];
                    s.d_hr[2] = s.d_hr[1];
                    s.d_hr[1] = input_sample_r;
                    input_sample_r = (s.d_hr[1] + s.d_hr[2] + s.d_hr[3]) / 3.0;
                    allpasstemp = s.alp_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.a_ir[allpasstemp as usize] * constallpass;
                    s.a_ir[s.alp_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ir -= 1;
                    if s.alp_ir < 0 || s.alp_ir > s.delay_i {
                        s.alp_ir = s.delay_i;
                    }
                    input_sample_r += s.a_ir[s.alp_ir as usize];
                    s.d_ir[3] = s.d_ir[2];
                    s.d_ir[2] = s.d_ir[1];
                    s.d_ir[1] = input_sample_r;
                    input_sample_r = (s.d_ir[1] + s.d_ir[2] + s.d_ir[3]) / 3.0;
                    allpasstemp = s.alp_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.a_jr[allpasstemp as usize] * constallpass;
                    s.a_jr[s.alp_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_jr -= 1;
                    if s.alp_jr < 0 || s.alp_jr > s.delay_j {
                        s.alp_jr = s.delay_j;
                    }
                    input_sample_r += s.a_jr[s.alp_jr as usize];
                    s.d_jr[3] = s.d_jr[2];
                    s.d_jr[2] = s.d_jr[1];
                    s.d_jr[1] = input_sample_r;
                    input_sample_r = (s.d_jr[1] + s.d_jr[2] + s.d_jr[3]) / 3.0;
                    allpasstemp = s.alp_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.a_kr[allpasstemp as usize] * constallpass;
                    s.a_kr[s.alp_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_kr -= 1;
                    if s.alp_kr < 0 || s.alp_kr > s.delay_k {
                        s.alp_kr = s.delay_k;
                    }
                    input_sample_r += s.a_kr[s.alp_kr as usize];
                    s.d_kr[3] = s.d_kr[2];
                    s.d_kr[2] = s.d_kr[1];
                    s.d_kr[1] = input_sample_r;
                    input_sample_r = (s.d_kr[1] + s.d_kr[2] + s.d_kr[3]) / 3.0;
                    allpasstemp = s.alp_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.a_lr[allpasstemp as usize] * constallpass;
                    s.a_lr[s.alp_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_lr -= 1;
                    if s.alp_lr < 0 || s.alp_lr > s.delay_l {
                        s.alp_lr = s.delay_l;
                    }
                    input_sample_r += s.a_lr[s.alp_lr as usize];
                    s.d_lr[3] = s.d_lr[2];
                    s.d_lr[2] = s.d_lr[1];
                    s.d_lr[1] = input_sample_r;
                    input_sample_r = (s.d_lr[1] + s.d_lr[2] + s.d_lr[3]) / 3.0;
                    allpasstemp = s.alp_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.a_mr[allpasstemp as usize] * constallpass;
                    s.a_mr[s.alp_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_mr -= 1;
                    if s.alp_mr < 0 || s.alp_mr > s.delay_m {
                        s.alp_mr = s.delay_m;
                    }
                    input_sample_r += s.a_mr[s.alp_mr as usize];
                    s.d_mr[3] = s.d_mr[2];
                    s.d_mr[2] = s.d_mr[1];
                    s.d_mr[1] = input_sample_r;
                    input_sample_r = (s.d_mr[1] + s.d_mr[2] + s.d_mr[3]) / 3.0;
                    allpasstemp = s.alp_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.a_nr[allpasstemp as usize] * constallpass;
                    s.a_nr[s.alp_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_nr -= 1;
                    if s.alp_nr < 0 || s.alp_nr > s.delay_n {
                        s.alp_nr = s.delay_n;
                    }
                    input_sample_r += s.a_nr[s.alp_nr as usize];
                    s.d_nr[3] = s.d_nr[2];
                    s.d_nr[2] = s.d_nr[1];
                    s.d_nr[1] = input_sample_r;
                    input_sample_r = (s.d_nr[1] + s.d_nr[2] + s.d_nr[3]) / 3.0;
                    allpasstemp = s.alp_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.a_or[allpasstemp as usize] * constallpass;
                    s.a_or[s.alp_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_or -= 1;
                    if s.alp_or < 0 || s.alp_or > s.delay_o {
                        s.alp_or = s.delay_o;
                    }
                    input_sample_r += s.a_or[s.alp_or as usize];
                    s.d_or[3] = s.d_or[2];
                    s.d_or[2] = s.d_or[1];
                    s.d_or[1] = input_sample_r;
                    input_sample_r = (s.d_or[1] + s.d_or[2] + s.d_or[3]) / 3.0;
                    allpasstemp = s.alp_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.a_pr[allpasstemp as usize] * constallpass;
                    s.a_pr[s.alp_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_pr -= 1;
                    if s.alp_pr < 0 || s.alp_pr > s.delay_p {
                        s.alp_pr = s.delay_p;
                    }
                    input_sample_r += s.a_pr[s.alp_pr as usize];
                    s.d_pr[3] = s.d_pr[2];
                    s.d_pr[2] = s.d_pr[1];
                    s.d_pr[1] = input_sample_r;
                    input_sample_r = (s.d_pr[1] + s.d_pr[2] + s.d_pr[3]) / 3.0;
                    allpasstemp = s.alp_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.a_qr[allpasstemp as usize] * constallpass;
                    s.a_qr[s.alp_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_qr -= 1;
                    if s.alp_qr < 0 || s.alp_qr > s.delay_q {
                        s.alp_qr = s.delay_q;
                    }
                    input_sample_r += s.a_qr[s.alp_qr as usize];
                    s.d_qr[3] = s.d_qr[2];
                    s.d_qr[2] = s.d_qr[1];
                    s.d_qr[1] = input_sample_r;
                    input_sample_r = (s.d_qr[1] + s.d_qr[2] + s.d_qr[3]) / 3.0;
                    allpasstemp = s.alp_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.a_rr[allpasstemp as usize] * constallpass;
                    s.a_rr[s.alp_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_rr -= 1;
                    if s.alp_rr < 0 || s.alp_rr > s.delay_r {
                        s.alp_rr = s.delay_r;
                    }
                    input_sample_r += s.a_rr[s.alp_rr as usize];
                    s.d_rr[3] = s.d_rr[2];
                    s.d_rr[2] = s.d_rr[1];
                    s.d_rr[1] = input_sample_r;
                    input_sample_r = (s.d_rr[1] + s.d_rr[2] + s.d_rr[3]) / 3.0;
                    allpasstemp = s.alp_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.a_sr[allpasstemp as usize] * constallpass;
                    s.a_sr[s.alp_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_sr -= 1;
                    if s.alp_sr < 0 || s.alp_sr > s.delay_s {
                        s.alp_sr = s.delay_s;
                    }
                    input_sample_r += s.a_sr[s.alp_sr as usize];
                    s.d_sr[3] = s.d_sr[2];
                    s.d_sr[2] = s.d_sr[1];
                    s.d_sr[1] = input_sample_r;
                    input_sample_r = (s.d_sr[1] + s.d_sr[2] + s.d_sr[3]) / 3.0;
                    allpasstemp = s.alp_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.a_tr[allpasstemp as usize] * constallpass;
                    s.a_tr[s.alp_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_tr -= 1;
                    if s.alp_tr < 0 || s.alp_tr > s.delay_t {
                        s.alp_tr = s.delay_t;
                    }
                    input_sample_r += s.a_tr[s.alp_tr as usize];
                    s.d_tr[3] = s.d_tr[2];
                    s.d_tr[2] = s.d_tr[1];
                    s.d_tr[1] = input_sample_r;
                    input_sample_r = (s.d_tr[1] + s.d_tr[2] + s.d_tr[3]) / 3.0;
                    allpasstemp = s.alp_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.a_ur[allpasstemp as usize] * constallpass;
                    s.a_ur[s.alp_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_ur -= 1;
                    if s.alp_ur < 0 || s.alp_ur > s.delay_u {
                        s.alp_ur = s.delay_u;
                    }
                    input_sample_r += s.a_ur[s.alp_ur as usize];
                    s.d_ur[3] = s.d_ur[2];
                    s.d_ur[2] = s.d_ur[1];
                    s.d_ur[1] = input_sample_r;
                    input_sample_r = (s.d_ur[1] + s.d_ur[2] + s.d_ur[3]) / 3.0;
                    allpasstemp = s.alp_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.a_vr[allpasstemp as usize] * constallpass;
                    s.a_vr[s.alp_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_vr -= 1;
                    if s.alp_vr < 0 || s.alp_vr > s.delay_v {
                        s.alp_vr = s.delay_v;
                    }
                    input_sample_r += s.a_vr[s.alp_vr as usize];
                    s.d_vr[3] = s.d_vr[2];
                    s.d_vr[2] = s.d_vr[1];
                    s.d_vr[1] = input_sample_r;
                    input_sample_r = (s.d_vr[1] + s.d_vr[2] + s.d_vr[3]) / 3.0;
                    allpasstemp = s.alp_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.a_wr[allpasstemp as usize] * constallpass;
                    s.a_wr[s.alp_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_wr -= 1;
                    if s.alp_wr < 0 || s.alp_wr > s.delay_w {
                        s.alp_wr = s.delay_w;
                    }
                    input_sample_r += s.a_wr[s.alp_wr as usize];
                    s.d_wr[3] = s.d_wr[2];
                    s.d_wr[2] = s.d_wr[1];
                    s.d_wr[1] = input_sample_r;
                    input_sample_r = (s.d_wr[1] + s.d_wr[2] + s.d_wr[3]) / 3.0;
                    allpasstemp = s.alp_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.a_xr[allpasstemp as usize] * constallpass;
                    s.a_xr[s.alp_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_xr -= 1;
                    if s.alp_xr < 0 || s.alp_xr > s.delay_x {
                        s.alp_xr = s.delay_x;
                    }
                    input_sample_r += s.a_xr[s.alp_xr as usize];
                    s.d_xr[3] = s.d_xr[2];
                    s.d_xr[2] = s.d_xr[1];
                    s.d_xr[1] = input_sample_r;
                    input_sample_r = (s.d_xr[1] + s.d_xr[2] + s.d_xr[3]) / 3.0;
                    allpasstemp = s.alp_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.a_yr[allpasstemp as usize] * constallpass;
                    s.a_yr[s.alp_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_yr -= 1;
                    if s.alp_yr < 0 || s.alp_yr > s.delay_y {
                        s.alp_yr = s.delay_y;
                    }
                    input_sample_r += s.a_yr[s.alp_yr as usize];
                    s.d_yr[3] = s.d_yr[2];
                    s.d_yr[2] = s.d_yr[1];
                    s.d_yr[1] = input_sample_r;
                    input_sample_r = (s.d_yr[1] + s.d_yr[2] + s.d_yr[3]) / 3.0;
                    allpasstemp = s.alp_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.a_zr[allpasstemp as usize] * constallpass;
                    s.a_zr[s.alp_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.alp_zr -= 1;
                    if s.alp_zr < 0 || s.alp_zr > s.delay_z {
                        s.alp_zr = s.delay_z;
                    }
                    input_sample_r += s.a_zr[s.alp_zr as usize];
                    s.d_zr[3] = s.d_zr[2];
                    s.d_zr[2] = s.d_zr[1];
                    s.d_zr[1] = input_sample_r;
                    input_sample_r = (s.d_zr[1] + s.d_zr[2] + s.d_zr[3]) / 3.0;
                    allpasstemp = s.out_ar - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_a {
                        allpasstemp = s.delay_a;
                    }
                    input_sample_r -= s.o_ar[allpasstemp as usize] * constallpass;
                    s.o_ar[s.out_ar as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ar -= 1;
                    if s.out_ar < 0 || s.out_ar > s.delay_a {
                        s.out_ar = s.delay_a;
                    }
                    input_sample_r += s.o_ar[s.out_ar as usize];
                    s.d_ar[6] = s.d_ar[5];
                    s.d_ar[5] = s.d_ar[4];
                    s.d_ar[4] = input_sample_r;
                    input_sample_r = (s.d_cr[1] + s.d_ar[5] + s.d_ar[6]) / 3.0;
                    allpasstemp = s.out_br - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_b {
                        allpasstemp = s.delay_b;
                    }
                    input_sample_r -= s.o_br[allpasstemp as usize] * constallpass;
                    s.o_br[s.out_br as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_br -= 1;
                    if s.out_br < 0 || s.out_br > s.delay_b {
                        s.out_br = s.delay_b;
                    }
                    input_sample_r += s.o_br[s.out_br as usize];
                    s.d_br[6] = s.d_br[5];
                    s.d_br[5] = s.d_br[4];
                    s.d_br[4] = input_sample_r;
                    input_sample_r = (s.d_br[4] + s.d_br[5] + s.d_br[6]) / 3.0;
                    allpasstemp = s.out_cr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_c {
                        allpasstemp = s.delay_c;
                    }
                    input_sample_r -= s.o_cr[allpasstemp as usize] * constallpass;
                    s.o_cr[s.out_cr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_cr -= 1;
                    if s.out_cr < 0 || s.out_cr > s.delay_c {
                        s.out_cr = s.delay_c;
                    }
                    input_sample_r += s.o_cr[s.out_cr as usize];
                    s.d_cr[6] = s.d_cr[5];
                    s.d_cr[5] = s.d_cr[4];
                    s.d_cr[4] = input_sample_r;
                    input_sample_r = (s.d_cr[4] + s.d_cr[5] + s.d_cr[6]) / 3.0;
                    allpasstemp = s.out_dr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_d {
                        allpasstemp = s.delay_d;
                    }
                    input_sample_r -= s.o_dr[allpasstemp as usize] * constallpass;
                    s.o_dr[s.out_dr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_dr -= 1;
                    if s.out_dr < 0 || s.out_dr > s.delay_d {
                        s.out_dr = s.delay_d;
                    }
                    input_sample_r += s.o_dr[s.out_dr as usize];
                    s.d_dr[6] = s.d_dr[5];
                    s.d_dr[5] = s.d_dr[4];
                    s.d_dr[4] = input_sample_r;
                    input_sample_r = (s.d_dr[4] + s.d_dr[5] + s.d_dr[6]) / 3.0;
                    allpasstemp = s.out_er - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_e {
                        allpasstemp = s.delay_e;
                    }
                    input_sample_r -= s.o_er[allpasstemp as usize] * constallpass;
                    s.o_er[s.out_er as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_er -= 1;
                    if s.out_er < 0 || s.out_er > s.delay_e {
                        s.out_er = s.delay_e;
                    }
                    input_sample_r += s.o_er[s.out_er as usize];
                    s.d_er[6] = s.d_er[5];
                    s.d_er[5] = s.d_er[4];
                    s.d_er[4] = input_sample_r;
                    input_sample_r = (s.d_er[4] + s.d_er[5] + s.d_er[6]) / 3.0;
                    allpasstemp = s.out_fr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_f {
                        allpasstemp = s.delay_f;
                    }
                    input_sample_r -= s.o_fr[allpasstemp as usize] * constallpass;
                    s.o_fr[s.out_fr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_fr -= 1;
                    if s.out_fr < 0 || s.out_fr > s.delay_f {
                        s.out_fr = s.delay_f;
                    }
                    input_sample_r += s.o_fr[s.out_fr as usize];
                    s.d_fr[6] = s.d_fr[5];
                    s.d_fr[5] = s.d_fr[4];
                    s.d_fr[4] = input_sample_r;
                    input_sample_r = (s.d_fr[4] + s.d_fr[5] + s.d_fr[6]) / 3.0;
                    allpasstemp = s.out_gr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_g {
                        allpasstemp = s.delay_g;
                    }
                    input_sample_r -= s.o_gr[allpasstemp as usize] * constallpass;
                    s.o_gr[s.out_gr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_gr -= 1;
                    if s.out_gr < 0 || s.out_gr > s.delay_g {
                        s.out_gr = s.delay_g;
                    }
                    input_sample_r += s.o_gr[s.out_gr as usize];
                    s.d_gr[6] = s.d_gr[5];
                    s.d_gr[5] = s.d_gr[4];
                    s.d_gr[4] = input_sample_r;
                    input_sample_r = (s.d_gr[4] + s.d_gr[5] + s.d_gr[6]) / 3.0;
                    allpasstemp = s.out_hr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_h {
                        allpasstemp = s.delay_h;
                    }
                    input_sample_r -= s.o_hr[allpasstemp as usize] * constallpass;
                    s.o_hr[s.out_hr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_hr -= 1;
                    if s.out_hr < 0 || s.out_hr > s.delay_h {
                        s.out_hr = s.delay_h;
                    }
                    input_sample_r += s.o_hr[s.out_hr as usize];
                    s.d_hr[6] = s.d_hr[5];
                    s.d_hr[5] = s.d_hr[4];
                    s.d_hr[4] = input_sample_r;
                    input_sample_r = (s.d_hr[4] + s.d_hr[5] + s.d_hr[6]) / 3.0;
                    allpasstemp = s.out_ir - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_i {
                        allpasstemp = s.delay_i;
                    }
                    input_sample_r -= s.o_ir[allpasstemp as usize] * constallpass;
                    s.o_ir[s.out_ir as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ir -= 1;
                    if s.out_ir < 0 || s.out_ir > s.delay_i {
                        s.out_ir = s.delay_i;
                    }
                    input_sample_r += s.o_ir[s.out_ir as usize];
                    s.d_ir[6] = s.d_ir[5];
                    s.d_ir[5] = s.d_ir[4];
                    s.d_ir[4] = input_sample_r;
                    input_sample_r = (s.d_ir[4] + s.d_ir[5] + s.d_ir[6]) / 3.0;
                    allpasstemp = s.out_jr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_j {
                        allpasstemp = s.delay_j;
                    }
                    input_sample_r -= s.o_jr[allpasstemp as usize] * constallpass;
                    s.o_jr[s.out_jr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_jr -= 1;
                    if s.out_jr < 0 || s.out_jr > s.delay_j {
                        s.out_jr = s.delay_j;
                    }
                    input_sample_r += s.o_jr[s.out_jr as usize];
                    s.d_jr[6] = s.d_jr[5];
                    s.d_jr[5] = s.d_jr[4];
                    s.d_jr[4] = input_sample_r;
                    input_sample_r = (s.d_jr[4] + s.d_jr[5] + s.d_jr[6]) / 3.0;
                    allpasstemp = s.out_kr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_k {
                        allpasstemp = s.delay_k;
                    }
                    input_sample_r -= s.o_kr[allpasstemp as usize] * constallpass;
                    s.o_kr[s.out_kr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_kr -= 1;
                    if s.out_kr < 0 || s.out_kr > s.delay_k {
                        s.out_kr = s.delay_k;
                    }
                    input_sample_r += s.o_kr[s.out_kr as usize];
                    s.d_kr[6] = s.d_kr[5];
                    s.d_kr[5] = s.d_kr[4];
                    s.d_kr[4] = input_sample_r;
                    input_sample_r = (s.d_kr[4] + s.d_kr[5] + s.d_kr[6]) / 3.0;
                    allpasstemp = s.out_lr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_l {
                        allpasstemp = s.delay_l;
                    }
                    input_sample_r -= s.o_lr[allpasstemp as usize] * constallpass;
                    s.o_lr[s.out_lr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_lr -= 1;
                    if s.out_lr < 0 || s.out_lr > s.delay_l {
                        s.out_lr = s.delay_l;
                    }
                    input_sample_r += s.o_lr[s.out_lr as usize];
                    s.d_lr[6] = s.d_lr[5];
                    s.d_lr[5] = s.d_lr[4];
                    s.d_lr[4] = input_sample_r;
                    input_sample_r = (s.d_lr[4] + s.d_lr[5] + s.d_lr[6]) / 3.0;
                    allpasstemp = s.out_mr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_m {
                        allpasstemp = s.delay_m;
                    }
                    input_sample_r -= s.o_mr[allpasstemp as usize] * constallpass;
                    s.o_mr[s.out_mr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_mr -= 1;
                    if s.out_mr < 0 || s.out_mr > s.delay_m {
                        s.out_mr = s.delay_m;
                    }
                    input_sample_r += s.o_mr[s.out_mr as usize];
                    s.d_mr[6] = s.d_mr[5];
                    s.d_mr[5] = s.d_mr[4];
                    s.d_mr[4] = input_sample_r;
                    input_sample_r = (s.d_mr[4] + s.d_mr[5] + s.d_mr[6]) / 3.0;
                    allpasstemp = s.out_nr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_n {
                        allpasstemp = s.delay_n;
                    }
                    input_sample_r -= s.o_nr[allpasstemp as usize] * constallpass;
                    s.o_nr[s.out_nr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_nr -= 1;
                    if s.out_nr < 0 || s.out_nr > s.delay_n {
                        s.out_nr = s.delay_n;
                    }
                    input_sample_r += s.o_nr[s.out_nr as usize];
                    s.d_nr[6] = s.d_nr[5];
                    s.d_nr[5] = s.d_nr[4];
                    s.d_nr[4] = input_sample_r;
                    input_sample_r = (s.d_nr[4] + s.d_nr[5] + s.d_nr[6]) / 3.0;
                    allpasstemp = s.out_or - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_o {
                        allpasstemp = s.delay_o;
                    }
                    input_sample_r -= s.o_or[allpasstemp as usize] * constallpass;
                    s.o_or[s.out_or as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_or -= 1;
                    if s.out_or < 0 || s.out_or > s.delay_o {
                        s.out_or = s.delay_o;
                    }
                    input_sample_r += s.o_or[s.out_or as usize];
                    s.d_or[6] = s.d_or[5];
                    s.d_or[5] = s.d_or[4];
                    s.d_or[4] = input_sample_r;
                    input_sample_r = (s.d_or[4] + s.d_or[5] + s.d_or[6]) / 3.0;
                    allpasstemp = s.out_pr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_p {
                        allpasstemp = s.delay_p;
                    }
                    input_sample_r -= s.o_pr[allpasstemp as usize] * constallpass;
                    s.o_pr[s.out_pr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_pr -= 1;
                    if s.out_pr < 0 || s.out_pr > s.delay_p {
                        s.out_pr = s.delay_p;
                    }
                    input_sample_r += s.o_pr[s.out_pr as usize];
                    s.d_pr[6] = s.d_pr[5];
                    s.d_pr[5] = s.d_pr[4];
                    s.d_pr[4] = input_sample_r;
                    input_sample_r = (s.d_pr[4] + s.d_pr[5] + s.d_pr[6]) / 3.0;
                    allpasstemp = s.out_qr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_q {
                        allpasstemp = s.delay_q;
                    }
                    input_sample_r -= s.o_qr[allpasstemp as usize] * constallpass;
                    s.o_qr[s.out_qr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_qr -= 1;
                    if s.out_qr < 0 || s.out_qr > s.delay_q {
                        s.out_qr = s.delay_q;
                    }
                    input_sample_r += s.o_qr[s.out_qr as usize];
                    s.d_qr[6] = s.d_qr[5];
                    s.d_qr[5] = s.d_qr[4];
                    s.d_qr[4] = input_sample_r;
                    input_sample_r = (s.d_qr[4] + s.d_qr[5] + s.d_qr[6]) / 3.0;
                    allpasstemp = s.out_rr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_r {
                        allpasstemp = s.delay_r;
                    }
                    input_sample_r -= s.o_rr[allpasstemp as usize] * constallpass;
                    s.o_rr[s.out_rr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_rr -= 1;
                    if s.out_rr < 0 || s.out_rr > s.delay_r {
                        s.out_rr = s.delay_r;
                    }
                    input_sample_r += s.o_rr[s.out_rr as usize];
                    s.d_rr[6] = s.d_rr[5];
                    s.d_rr[5] = s.d_rr[4];
                    s.d_rr[4] = input_sample_r;
                    input_sample_r = (s.d_rr[4] + s.d_rr[5] + s.d_rr[6]) / 3.0;
                    allpasstemp = s.out_sr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_s {
                        allpasstemp = s.delay_s;
                    }
                    input_sample_r -= s.o_sr[allpasstemp as usize] * constallpass;
                    s.o_sr[s.out_sr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_sr -= 1;
                    if s.out_sr < 0 || s.out_sr > s.delay_s {
                        s.out_sr = s.delay_s;
                    }
                    input_sample_r += s.o_sr[s.out_sr as usize];
                    s.d_sr[6] = s.d_sr[5];
                    s.d_sr[5] = s.d_sr[4];
                    s.d_sr[4] = input_sample_r;
                    input_sample_r = (s.d_sr[4] + s.d_sr[5] + s.d_sr[6]) / 3.0;
                    allpasstemp = s.out_tr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_t {
                        allpasstemp = s.delay_t;
                    }
                    input_sample_r -= s.o_tr[allpasstemp as usize] * constallpass;
                    s.o_tr[s.out_tr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_tr -= 1;
                    if s.out_tr < 0 || s.out_tr > s.delay_t {
                        s.out_tr = s.delay_t;
                    }
                    input_sample_r += s.o_tr[s.out_tr as usize];
                    s.d_tr[6] = s.d_tr[5];
                    s.d_tr[5] = s.d_tr[4];
                    s.d_tr[4] = input_sample_r;
                    input_sample_r = (s.d_tr[4] + s.d_tr[5] + s.d_tr[6]) / 3.0;
                    allpasstemp = s.out_ur - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_u {
                        allpasstemp = s.delay_u;
                    }
                    input_sample_r -= s.o_ur[allpasstemp as usize] * constallpass;
                    s.o_ur[s.out_ur as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_ur -= 1;
                    if s.out_ur < 0 || s.out_ur > s.delay_u {
                        s.out_ur = s.delay_u;
                    }
                    input_sample_r += s.o_ur[s.out_ur as usize];
                    s.d_ur[6] = s.d_ur[5];
                    s.d_ur[5] = s.d_ur[4];
                    s.d_ur[4] = input_sample_r;
                    input_sample_r = (s.d_ur[4] + s.d_ur[5] + s.d_ur[6]) / 3.0;
                    allpasstemp = s.out_vr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_v {
                        allpasstemp = s.delay_v;
                    }
                    input_sample_r -= s.o_vr[allpasstemp as usize] * constallpass;
                    s.o_vr[s.out_vr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_vr -= 1;
                    if s.out_vr < 0 || s.out_vr > s.delay_v {
                        s.out_vr = s.delay_v;
                    }
                    input_sample_r += s.o_vr[s.out_vr as usize];
                    s.d_vr[6] = s.d_vr[5];
                    s.d_vr[5] = s.d_vr[4];
                    s.d_vr[4] = input_sample_r;
                    input_sample_r = (s.d_vr[4] + s.d_vr[5] + s.d_vr[6]) / 3.0;
                    allpasstemp = s.out_wr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_w {
                        allpasstemp = s.delay_w;
                    }
                    input_sample_r -= s.o_wr[allpasstemp as usize] * constallpass;
                    s.o_wr[s.out_wr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_wr -= 1;
                    if s.out_wr < 0 || s.out_wr > s.delay_w {
                        s.out_wr = s.delay_w;
                    }
                    input_sample_r += s.o_wr[s.out_wr as usize];
                    s.d_wr[6] = s.d_wr[5];
                    s.d_wr[5] = s.d_wr[4];
                    s.d_wr[4] = input_sample_r;
                    input_sample_r = (s.d_wr[4] + s.d_wr[5] + s.d_wr[6]) / 3.0;
                    allpasstemp = s.out_xr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_x {
                        allpasstemp = s.delay_x;
                    }
                    input_sample_r -= s.o_xr[allpasstemp as usize] * constallpass;
                    s.o_xr[s.out_xr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_xr -= 1;
                    if s.out_xr < 0 || s.out_xr > s.delay_x {
                        s.out_xr = s.delay_x;
                    }
                    input_sample_r += s.o_xr[s.out_xr as usize];
                    s.d_xr[6] = s.d_xr[5];
                    s.d_xr[5] = s.d_xr[4];
                    s.d_xr[4] = input_sample_r;
                    input_sample_r = (s.d_xr[4] + s.d_xr[5] + s.d_xr[6]) / 3.0;
                    allpasstemp = s.out_yr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_y {
                        allpasstemp = s.delay_y;
                    }
                    input_sample_r -= s.o_yr[allpasstemp as usize] * constallpass;
                    s.o_yr[s.out_yr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_yr -= 1;
                    if s.out_yr < 0 || s.out_yr > s.delay_y {
                        s.out_yr = s.delay_y;
                    }
                    input_sample_r += s.o_yr[s.out_yr as usize];
                    s.d_yr[6] = s.d_yr[5];
                    s.d_yr[5] = s.d_yr[4];
                    s.d_yr[4] = input_sample_r;
                    input_sample_r = (s.d_yr[4] + s.d_yr[5] + s.d_yr[6]) / 3.0;
                    allpasstemp = s.out_zr - 1;
                    if allpasstemp < 0 || allpasstemp > s.delay_z {
                        allpasstemp = s.delay_z;
                    }
                    input_sample_r -= s.o_zr[allpasstemp as usize] * constallpass;
                    s.o_zr[s.out_zr as usize] = input_sample_r;
                    input_sample_r *= constallpass;
                    s.out_zr -= 1;
                    if s.out_zr < 0 || s.out_zr > s.delay_z {
                        s.out_zr = s.delay_z;
                    }
                    input_sample_r += s.o_zr[s.out_zr as usize];
                    s.d_zr[6] = s.d_zr[5];
                    s.d_zr[5] = s.d_zr[4];
                    s.d_zr[4] = input_sample_r;
                    input_sample_r = s.d_zr[4] + s.d_zr[5] + s.d_zr[6];
                }
                _ => {}
            }
            bridgerectifier = input_sample_l.abs();
            bridgerectifier = 1.0 - bridgerectifier.cos();
            if input_sample_l > 0.0 {
                input_sample_l -= bridgerectifier;
            } else {
                input_sample_l += bridgerectifier;
            }
            input_sample_l /= gain;
            bridgerectifier = input_sample_r.abs();
            bridgerectifier = 1.0 - bridgerectifier.cos();
            if input_sample_r > 0.0 {
                input_sample_r -= bridgerectifier;
            } else {
                input_sample_r += bridgerectifier;
            }
            input_sample_r /= gain;
            wetness = wetnesstarget;
            wetness *= s.peak_l;
            dry_sample_l *= 1.0 - wetness;
            input_sample_l *= wetness;
            input_sample_l += dry_sample_l;
            wetness = wetnesstarget;
            wetness *= s.peak_r;
            dry_sample_r *= 1.0 - wetness;
            input_sample_r *= wetness;
            input_sample_r += dry_sample_r;
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
