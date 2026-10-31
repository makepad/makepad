// Ported from NonlinearSpace (NonlinearSpaceProc.cpp, NonlinearSpace.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Nonlinear Space (sub-effect id 64): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("SmpRate", 0.3),
    pf("Livenes", 0.5),
    pf("Treble", 0.5),
    pf("Bass", 0.5),
    pf("Nonlin", 0.5),
    pf("Dry/Wet", 1.0),
];

#[allow(dead_code)]
pub struct NonlinearSpace {
    p: [f32; 6],
    sr: f32,
    avg_input_l: f64,
    avg_output_l: f64,
    avg2_input_l: f64,
    avg2_output_l: f64,
    avg_input_r: f64,
    avg_output_r: f64,
    avg2_input_r: f64,
    avg2_output_r: f64,
    a2vg_input_l: f64,
    a2vg_output_l: f64,
    a2vg2_input_l: f64,
    a2vg2_output_l: f64,
    a2vg_input_r: f64,
    a2vg_output_r: f64,
    a2vg2_input_r: f64,
    a2vg2_output_r: f64,
    verbout_l: f64,
    verbout_r: f64,
    iir_ccsample_l: f64,
    iir_ccsample_r: f64,
    iir_sample_l: f64,
    iir_sample_r: f64,
    d_mid: Vec<f64>,
    d_side: Vec<f64>,
    d_left: Vec<f64>,
    d_right: Vec<f64>,
    dpre_r: Vec<f64>,
    dpre_l: Vec<f64>,
    d_a: Vec<f64>,
    d_b: Vec<f64>,
    d_c: Vec<f64>,
    d_d: Vec<f64>,
    d_e: Vec<f64>,
    d_f: Vec<f64>,
    d_g: Vec<f64>,
    d_h: Vec<f64>,
    d_i: Vec<f64>,
    d_j: Vec<f64>,
    d_k: Vec<f64>,
    d_l: Vec<f64>,
    d_m: Vec<f64>,
    d_n: Vec<f64>,
    d_o: Vec<f64>,
    d_p: Vec<f64>,
    d_q: Vec<f64>,
    d_r: Vec<f64>,
    d_s: Vec<f64>,
    d_t: Vec<f64>,
    d_u: Vec<f64>,
    d_v: Vec<f64>,
    d_w: Vec<f64>,
    d_x: Vec<f64>,
    d_y: Vec<f64>,
    d_z: Vec<f64>,
    interpol_a: f64,
    pitchshift_a: f64,
    interpol_b: f64,
    pitchshift_b: f64,
    interpol_c: f64,
    pitchshift_c: f64,
    interpol_d: f64,
    pitchshift_d: f64,
    interpol_e: f64,
    pitchshift_e: f64,
    interpol_f: f64,
    pitchshift_f: f64,
    interpol_g: f64,
    pitchshift_g: f64,
    interpol_h: f64,
    pitchshift_h: f64,
    interpol_i: f64,
    pitchshift_i: f64,
    interpol_j: f64,
    pitchshift_j: f64,
    interpol_k: f64,
    pitchshift_k: f64,
    interpol_l: f64,
    pitchshift_l: f64,
    interpol_m: f64,
    pitchshift_m: f64,
    interpol_n: f64,
    pitchshift_n: f64,
    interpol_o: f64,
    pitchshift_o: f64,
    interpol_p: f64,
    pitchshift_p: f64,
    interpol_q: f64,
    pitchshift_q: f64,
    interpol_r: f64,
    pitchshift_r: f64,
    interpol_s: f64,
    pitchshift_s: f64,
    interpol_t: f64,
    pitchshift_t: f64,
    interpol_u: f64,
    pitchshift_u: f64,
    interpol_v: f64,
    pitchshift_v: f64,
    interpol_w: f64,
    pitchshift_w: f64,
    interpol_x: f64,
    pitchshift_x: f64,
    interpol_y: f64,
    pitchshift_y: f64,
    interpol_z: f64,
    pitchshift_z: f64,
    one_mid: i32,
    delay_mid: i32,
    maxdelay_mid: i32,
    one_side: i32,
    delay_side: i32,
    maxdelay_side: i32,
    one_left: i32,
    delay_left: i32,
    maxdelay_left: i32,
    one_right: i32,
    delay_right: i32,
    maxdelay_right: i32,
    onepre: i32,
    delaypre: i32,
    maxdelaypre: i32,
    one_a: i32,
    two_a: i32,
    tre_a: i32,
    delay_a: i32,
    maxdelay_a: i32,
    one_b: i32,
    two_b: i32,
    tre_b: i32,
    delay_b: i32,
    maxdelay_b: i32,
    one_c: i32,
    two_c: i32,
    tre_c: i32,
    delay_c: i32,
    maxdelay_c: i32,
    one_d: i32,
    two_d: i32,
    tre_d: i32,
    delay_d: i32,
    maxdelay_d: i32,
    one_e: i32,
    two_e: i32,
    tre_e: i32,
    delay_e: i32,
    maxdelay_e: i32,
    one_f: i32,
    two_f: i32,
    tre_f: i32,
    delay_f: i32,
    maxdelay_f: i32,
    one_g: i32,
    two_g: i32,
    tre_g: i32,
    delay_g: i32,
    maxdelay_g: i32,
    one_h: i32,
    two_h: i32,
    tre_h: i32,
    delay_h: i32,
    maxdelay_h: i32,
    one_i: i32,
    two_i: i32,
    tre_i: i32,
    delay_i: i32,
    maxdelay_i: i32,
    one_j: i32,
    two_j: i32,
    tre_j: i32,
    delay_j: i32,
    maxdelay_j: i32,
    one_k: i32,
    two_k: i32,
    tre_k: i32,
    delay_k: i32,
    maxdelay_k: i32,
    one_l: i32,
    two_l: i32,
    tre_l: i32,
    delay_l: i32,
    maxdelay_l: i32,
    one_m: i32,
    two_m: i32,
    tre_m: i32,
    delay_m: i32,
    maxdelay_m: i32,
    one_n: i32,
    two_n: i32,
    tre_n: i32,
    delay_n: i32,
    maxdelay_n: i32,
    one_o: i32,
    two_o: i32,
    tre_o: i32,
    delay_o: i32,
    maxdelay_o: i32,
    one_p: i32,
    two_p: i32,
    tre_p: i32,
    delay_p: i32,
    maxdelay_p: i32,
    one_q: i32,
    two_q: i32,
    tre_q: i32,
    delay_q: i32,
    maxdelay_q: i32,
    one_r: i32,
    two_r: i32,
    tre_r: i32,
    delay_r: i32,
    maxdelay_r: i32,
    one_s: i32,
    two_s: i32,
    tre_s: i32,
    delay_s: i32,
    maxdelay_s: i32,
    one_t: i32,
    two_t: i32,
    tre_t: i32,
    delay_t: i32,
    maxdelay_t: i32,
    one_u: i32,
    two_u: i32,
    tre_u: i32,
    delay_u: i32,
    maxdelay_u: i32,
    one_v: i32,
    two_v: i32,
    tre_v: i32,
    delay_v: i32,
    maxdelay_v: i32,
    one_w: i32,
    two_w: i32,
    tre_w: i32,
    delay_w: i32,
    maxdelay_w: i32,
    one_x: i32,
    two_x: i32,
    tre_x: i32,
    delay_x: i32,
    maxdelay_x: i32,
    one_y: i32,
    two_y: i32,
    tre_y: i32,
    delay_y: i32,
    maxdelay_y: i32,
    one_z: i32,
    two_z: i32,
    tre_z: i32,
    delay_z: i32,
    maxdelay_z: i32,
    saved_predelay: f64,
    saved_roomsize: f64,
    countdown: i32,
    lowpass_sample_aa: f64,
    lowpass_sample_ab: f64,
    lowpass_sample_ba: f64,
    lowpass_sample_bb: f64,
    lowpass_sample_ca: f64,
    lowpass_sample_cb: f64,
    lowpass_sample_da: f64,
    lowpass_sample_db: f64,
    lowpass_sample_e: f64,
    lowpass_sample_f: f64,
    lowpass_sample_g: f64,
    rowpass_sample_aa: f64,
    rowpass_sample_ab: f64,
    rowpass_sample_ba: f64,
    rowpass_sample_bb: f64,
    rowpass_sample_ca: f64,
    rowpass_sample_cb: f64,
    rowpass_sample_da: f64,
    rowpass_sample_db: f64,
    rowpass_sample_e: f64,
    rowpass_sample_f: f64,
    rowpass_sample_g: f64,
    flip: bool,
    nonlin: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl NonlinearSpace {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<NonlinearSpace> {
        let mut s = Box::new(NonlinearSpace {
            p: [0.3, 0.5, 0.5, 0.5, 0.5, 1.0],
            sr: sr as f32,
            avg_input_l: 0.0,
            avg_output_l: 0.0,
            avg2_input_l: 0.0,
            avg2_output_l: 0.0,
            avg_input_r: 0.0,
            avg_output_r: 0.0,
            avg2_input_r: 0.0,
            avg2_output_r: 0.0,
            a2vg_input_l: 0.0,
            a2vg_output_l: 0.0,
            a2vg2_input_l: 0.0,
            a2vg2_output_l: 0.0,
            a2vg_input_r: 0.0,
            a2vg_output_r: 0.0,
            a2vg2_input_r: 0.0,
            a2vg2_output_r: 0.0,
            verbout_l: 0.0,
            verbout_r: 0.0,
            iir_ccsample_l: 0.0,
            iir_ccsample_r: 0.0,
            iir_sample_l: 0.0,
            iir_sample_r: 0.0,
            d_mid: vec![0.0; 2348],
            d_side: vec![0.0; 1334],
            d_left: vec![0.0; 5924],
            d_right: vec![0.0; 5926],
            dpre_r: vec![0.0; 7575],
            dpre_l: vec![0.0; 7575],
            d_a: vec![0.0; 7575],
            d_b: vec![0.0; 7309],
            d_c: vec![0.0; 7179],
            d_d: vec![0.0; 6909],
            d_e: vec![0.0; 6781],
            d_f: vec![0.0; 6523],
            d_g: vec![0.0; 5983],
            d_h: vec![0.0; 5565],
            d_i: vec![0.0; 5299],
            d_j: vec![0.0; 4905],
            d_k: vec![0.0; 4761],
            d_l: vec![0.0; 4491],
            d_m: vec![0.0; 4393],
            d_n: vec![0.0; 4231],
            d_o: vec![0.0; 4155],
            d_p: vec![0.0; 3991],
            d_q: vec![0.0; 3661],
            d_r: vec![0.0; 3409],
            d_s: vec![0.0; 3253],
            d_t: vec![0.0; 3001],
            d_u: vec![0.0; 2919],
            d_v: vec![0.0; 2751],
            d_w: vec![0.0; 2505],
            d_x: vec![0.0; 2425],
            d_y: vec![0.0; 2148],
            d_z: vec![0.0; 2090],
            interpol_a: 0.0,
            pitchshift_a: 0.0,
            interpol_b: 0.0,
            pitchshift_b: 0.0,
            interpol_c: 0.0,
            pitchshift_c: 0.0,
            interpol_d: 0.0,
            pitchshift_d: 0.0,
            interpol_e: 0.0,
            pitchshift_e: 0.0,
            interpol_f: 0.0,
            pitchshift_f: 0.0,
            interpol_g: 0.0,
            pitchshift_g: 0.0,
            interpol_h: 0.0,
            pitchshift_h: 0.0,
            interpol_i: 0.0,
            pitchshift_i: 0.0,
            interpol_j: 0.0,
            pitchshift_j: 0.0,
            interpol_k: 0.0,
            pitchshift_k: 0.0,
            interpol_l: 0.0,
            pitchshift_l: 0.0,
            interpol_m: 0.0,
            pitchshift_m: 0.0,
            interpol_n: 0.0,
            pitchshift_n: 0.0,
            interpol_o: 0.0,
            pitchshift_o: 0.0,
            interpol_p: 0.0,
            pitchshift_p: 0.0,
            interpol_q: 0.0,
            pitchshift_q: 0.0,
            interpol_r: 0.0,
            pitchshift_r: 0.0,
            interpol_s: 0.0,
            pitchshift_s: 0.0,
            interpol_t: 0.0,
            pitchshift_t: 0.0,
            interpol_u: 0.0,
            pitchshift_u: 0.0,
            interpol_v: 0.0,
            pitchshift_v: 0.0,
            interpol_w: 0.0,
            pitchshift_w: 0.0,
            interpol_x: 0.0,
            pitchshift_x: 0.0,
            interpol_y: 0.0,
            pitchshift_y: 0.0,
            interpol_z: 0.0,
            pitchshift_z: 0.0,
            one_mid: 0,
            delay_mid: 0,
            maxdelay_mid: 0,
            one_side: 0,
            delay_side: 0,
            maxdelay_side: 0,
            one_left: 0,
            delay_left: 0,
            maxdelay_left: 0,
            one_right: 0,
            delay_right: 0,
            maxdelay_right: 0,
            onepre: 0,
            delaypre: 0,
            maxdelaypre: 0,
            one_a: 0,
            two_a: 0,
            tre_a: 0,
            delay_a: 0,
            maxdelay_a: 0,
            one_b: 0,
            two_b: 0,
            tre_b: 0,
            delay_b: 0,
            maxdelay_b: 0,
            one_c: 0,
            two_c: 0,
            tre_c: 0,
            delay_c: 0,
            maxdelay_c: 0,
            one_d: 0,
            two_d: 0,
            tre_d: 0,
            delay_d: 0,
            maxdelay_d: 0,
            one_e: 0,
            two_e: 0,
            tre_e: 0,
            delay_e: 0,
            maxdelay_e: 0,
            one_f: 0,
            two_f: 0,
            tre_f: 0,
            delay_f: 0,
            maxdelay_f: 0,
            one_g: 0,
            two_g: 0,
            tre_g: 0,
            delay_g: 0,
            maxdelay_g: 0,
            one_h: 0,
            two_h: 0,
            tre_h: 0,
            delay_h: 0,
            maxdelay_h: 0,
            one_i: 0,
            two_i: 0,
            tre_i: 0,
            delay_i: 0,
            maxdelay_i: 0,
            one_j: 0,
            two_j: 0,
            tre_j: 0,
            delay_j: 0,
            maxdelay_j: 0,
            one_k: 0,
            two_k: 0,
            tre_k: 0,
            delay_k: 0,
            maxdelay_k: 0,
            one_l: 0,
            two_l: 0,
            tre_l: 0,
            delay_l: 0,
            maxdelay_l: 0,
            one_m: 0,
            two_m: 0,
            tre_m: 0,
            delay_m: 0,
            maxdelay_m: 0,
            one_n: 0,
            two_n: 0,
            tre_n: 0,
            delay_n: 0,
            maxdelay_n: 0,
            one_o: 0,
            two_o: 0,
            tre_o: 0,
            delay_o: 0,
            maxdelay_o: 0,
            one_p: 0,
            two_p: 0,
            tre_p: 0,
            delay_p: 0,
            maxdelay_p: 0,
            one_q: 0,
            two_q: 0,
            tre_q: 0,
            delay_q: 0,
            maxdelay_q: 0,
            one_r: 0,
            two_r: 0,
            tre_r: 0,
            delay_r: 0,
            maxdelay_r: 0,
            one_s: 0,
            two_s: 0,
            tre_s: 0,
            delay_s: 0,
            maxdelay_s: 0,
            one_t: 0,
            two_t: 0,
            tre_t: 0,
            delay_t: 0,
            maxdelay_t: 0,
            one_u: 0,
            two_u: 0,
            tre_u: 0,
            delay_u: 0,
            maxdelay_u: 0,
            one_v: 0,
            two_v: 0,
            tre_v: 0,
            delay_v: 0,
            maxdelay_v: 0,
            one_w: 0,
            two_w: 0,
            tre_w: 0,
            delay_w: 0,
            maxdelay_w: 0,
            one_x: 0,
            two_x: 0,
            tre_x: 0,
            delay_x: 0,
            maxdelay_x: 0,
            one_y: 0,
            two_y: 0,
            tre_y: 0,
            delay_y: 0,
            maxdelay_y: 0,
            one_z: 0,
            two_z: 0,
            tre_z: 0,
            delay_z: 0,
            maxdelay_z: 0,
            saved_predelay: 0.0,
            saved_roomsize: 0.0,
            countdown: 0,
            lowpass_sample_aa: 0.0,
            lowpass_sample_ab: 0.0,
            lowpass_sample_ba: 0.0,
            lowpass_sample_bb: 0.0,
            lowpass_sample_ca: 0.0,
            lowpass_sample_cb: 0.0,
            lowpass_sample_da: 0.0,
            lowpass_sample_db: 0.0,
            lowpass_sample_e: 0.0,
            lowpass_sample_f: 0.0,
            lowpass_sample_g: 0.0,
            rowpass_sample_aa: 0.0,
            rowpass_sample_ab: 0.0,
            rowpass_sample_ba: 0.0,
            rowpass_sample_bb: 0.0,
            rowpass_sample_ca: 0.0,
            rowpass_sample_cb: 0.0,
            rowpass_sample_da: 0.0,
            rowpass_sample_db: 0.0,
            rowpass_sample_e: 0.0,
            rowpass_sample_f: 0.0,
            rowpass_sample_g: 0.0,
            flip: false,
            nonlin: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        let mut count: i32 = 0;
        s.one_mid = 1;
        s.delay_mid = 2346;
        s.maxdelay_mid = 2346;
        s.one_side = 1;
        s.delay_side = 1332;
        s.maxdelay_side = 1332;
        s.one_left = 1;
        s.delay_left = 5922;
        s.maxdelay_left = 5922;
        s.one_right = 1;
        s.delay_right = 5924;
        s.maxdelay_right = 5924;
        s.onepre = 1;
        s.delaypre = 7573;
        s.maxdelaypre = 7573;
        s.one_a = 1;
        s.two_a = 2;
        s.tre_a = 3;
        s.delay_a = 7573;
        s.maxdelay_a = 7573;
        s.one_b = 1;
        s.two_b = 2;
        s.tre_b = 3;
        s.delay_b = 7307;
        s.maxdelay_b = 7307;
        s.one_c = 1;
        s.two_c = 2;
        s.tre_c = 3;
        s.delay_c = 7177;
        s.maxdelay_c = 7177;
        s.one_d = 1;
        s.two_d = 2;
        s.tre_d = 3;
        s.delay_d = 6907;
        s.maxdelay_d = 6907;
        s.one_e = 1;
        s.two_e = 2;
        s.tre_e = 3;
        s.delay_e = 6779;
        s.maxdelay_e = 6779;
        s.one_f = 1;
        s.two_f = 2;
        s.tre_f = 3;
        s.delay_f = 6521;
        s.maxdelay_f = 6521;
        s.one_g = 1;
        s.two_g = 2;
        s.tre_g = 3;
        s.delay_g = 5981;
        s.maxdelay_g = 5981;
        s.one_h = 1;
        s.two_h = 2;
        s.tre_h = 3;
        s.delay_h = 5563;
        s.maxdelay_h = 5563;
        s.one_i = 1;
        s.two_i = 2;
        s.tre_i = 3;
        s.delay_i = 5297;
        s.maxdelay_i = 5297;
        s.one_j = 1;
        s.two_j = 2;
        s.tre_j = 3;
        s.delay_j = 4903;
        s.maxdelay_j = 4903;
        s.one_k = 1;
        s.two_k = 2;
        s.tre_k = 3;
        s.delay_k = 4759;
        s.maxdelay_k = 4759;
        s.one_l = 1;
        s.two_l = 2;
        s.tre_l = 3;
        s.delay_l = 4489;
        s.maxdelay_l = 4489;
        s.one_m = 1;
        s.two_m = 2;
        s.tre_m = 3;
        s.delay_m = 4391;
        s.maxdelay_m = 4391;
        s.one_n = 1;
        s.two_n = 2;
        s.tre_n = 3;
        s.delay_n = 4229;
        s.maxdelay_n = 4229;
        s.one_o = 1;
        s.two_o = 2;
        s.tre_o = 3;
        s.delay_o = 4153;
        s.maxdelay_o = 4153;
        s.one_p = 1;
        s.two_p = 2;
        s.tre_p = 3;
        s.delay_p = 3989;
        s.maxdelay_p = 3989;
        s.one_q = 1;
        s.two_q = 2;
        s.tre_q = 3;
        s.delay_q = 3659;
        s.maxdelay_q = 3659;
        s.one_r = 1;
        s.two_r = 2;
        s.tre_r = 3;
        s.delay_r = 3407;
        s.maxdelay_r = 3407;
        s.one_s = 1;
        s.two_s = 2;
        s.tre_s = 3;
        s.delay_s = 3251;
        s.maxdelay_s = 3251;
        s.one_t = 1;
        s.two_t = 2;
        s.tre_t = 3;
        s.delay_t = 2999;
        s.maxdelay_t = 2999;
        s.one_u = 1;
        s.two_u = 2;
        s.tre_u = 3;
        s.delay_u = 2917;
        s.maxdelay_u = 2917;
        s.one_v = 1;
        s.two_v = 2;
        s.tre_v = 3;
        s.delay_v = 2749;
        s.maxdelay_v = 2749;
        s.one_w = 1;
        s.two_w = 2;
        s.tre_w = 3;
        s.delay_w = 2503;
        s.maxdelay_w = 2503;
        s.one_x = 1;
        s.two_x = 2;
        s.tre_x = 3;
        s.delay_x = 2423;
        s.maxdelay_x = 2423;
        s.one_y = 1;
        s.two_y = 2;
        s.tre_y = 3;
        s.delay_y = 2146;
        s.maxdelay_y = 2146;
        s.one_z = 1;
        s.two_z = 2;
        s.tre_z = 3;
        s.delay_z = 2088;
        s.maxdelay_z = 2088;
        s.pitchshift_a = 1.0 / s.maxdelay_a as f64;
        s.pitchshift_b = 1.0 / s.maxdelay_b as f64;
        s.pitchshift_c = 1.0 / s.maxdelay_c as f64;
        s.pitchshift_d = 1.0 / s.maxdelay_d as f64;
        s.pitchshift_e = 1.0 / s.maxdelay_e as f64;
        s.pitchshift_f = 1.0 / s.maxdelay_f as f64;
        s.pitchshift_g = 1.0 / s.maxdelay_g as f64;
        s.pitchshift_h = 1.0 / s.maxdelay_h as f64;
        s.pitchshift_i = 1.0 / s.maxdelay_i as f64;
        s.pitchshift_j = 1.0 / s.maxdelay_j as f64;
        s.pitchshift_k = 1.0 / s.maxdelay_k as f64;
        s.pitchshift_l = 1.0 / s.maxdelay_l as f64;
        s.pitchshift_m = 1.0 / s.maxdelay_m as f64;
        s.pitchshift_n = 1.0 / s.maxdelay_n as f64;
        s.pitchshift_o = 1.0 / s.maxdelay_o as f64;
        s.pitchshift_p = 1.0 / s.maxdelay_p as f64;
        s.pitchshift_q = 1.0 / s.maxdelay_q as f64;
        s.pitchshift_r = 1.0 / s.maxdelay_r as f64;
        s.pitchshift_s = 1.0 / s.maxdelay_s as f64;
        s.pitchshift_t = 1.0 / s.maxdelay_t as f64;
        s.pitchshift_u = 1.0 / s.maxdelay_u as f64;
        s.pitchshift_v = 1.0 / s.maxdelay_v as f64;
        s.pitchshift_w = 1.0 / s.maxdelay_w as f64;
        s.pitchshift_x = 1.0 / s.maxdelay_x as f64;
        s.pitchshift_y = 1.0 / s.maxdelay_y as f64;
        s.pitchshift_z = 1.0 / s.maxdelay_z as f64;
        s.saved_roomsize = -1.0;
        s.countdown = -1;
        s.flip = true;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for NonlinearSpace {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut dry_sample_l: f64 = 0.0;
        let mut dry_sample_r: f64 = 0.0;
        let mut input_sample_l: f64 = 0.0;
        let mut input_sample_r: f64 = 0.0;
        let mut mid: f64 = 0.0;
        let mut side: f64 = 0.0;
        let mut overallscale: f64 = 1.0;
        let mut samplerate: i32 = (s.p[0] as f64 * 6.999) as i32 + 1;
        match samplerate {
            1 => {
                overallscale *= 16.0 / 44.1;
            }
            2 => {
                overallscale *= 32.0 / 44.1;
            }
            3 => {
                overallscale *= 1.0;
            }
            4 => {
                overallscale *= 48.0 / 44.1;
            }
            5 => {
                overallscale *= 64.0 / 44.1;
            }
            6 => {
                overallscale *= 2.0;
            }
            7 => {
                overallscale *= 96.0 / 44.1;
            }
            _ => {}
        }
        s.nonlin *= 0.001;
        let mut basefeedback: f64 = 0.45 + s.nonlin * (s.p[4] as f64 * 2.0 - 1.0).powf(3.0);
        s.nonlin = 0.0;
        let mut tankfeedback: f64 = basefeedback + (s.p[1] as f64).powf(2.0) * 0.05;
        if tankfeedback > 0.5 {
            tankfeedback = 0.5;
        }
        if tankfeedback < 0.4 {
            tankfeedback = 0.4;
        }
        let mut iir_amount_c: f64 = 1.0 - (1.0 - s.p[2] as f64).powf(2.0);
        iir_amount_c += iir_amount_c / overallscale;
        iir_amount_c /= 2.0;
        if iir_amount_c > 1.1 {
            iir_amount_c = 1.1;
        }
        let mut iir_amount: f64 = (1.0 - (s.p[3] as f64).powf(2.0)) * 0.09 / overallscale + 0.001;
        if iir_amount > 1.0 {
            iir_amount = 1.0;
        }
        if iir_amount < 0.001 {
            iir_amount = 0.001;
        }
        let mut wetness: f64 = s.p[5] as f64;
        let mut dryness: f64 = 1.0 - wetness;
        let mut roomsize: f64 = overallscale * 0.203;
        let mut lean: f64 = 0.125;
        let mut invlean: f64 = 1.0 - lean;
        let mut pspeed: f64 = 0.145;
        let mut outcouple: f64 = 0.5 - tankfeedback;
        let mut constallpass: f64 = 0.618033988749894848204586;
        let mut temp: f64 = 0.0;
        let mut allpasstemp: i32 = 0;
        let mut predelay: f64 = 0.222 * overallscale;
        s.delay_a = (s.maxdelay_a as f64 * roomsize) as i32;
        s.delay_b = (s.maxdelay_b as f64 * roomsize) as i32;
        s.delay_c = (s.maxdelay_c as f64 * roomsize) as i32;
        s.delay_d = (s.maxdelay_d as f64 * roomsize) as i32;
        s.delay_e = (s.maxdelay_e as f64 * roomsize) as i32;
        s.delay_f = (s.maxdelay_f as f64 * roomsize) as i32;
        s.delay_g = (s.maxdelay_g as f64 * roomsize) as i32;
        s.delay_h = (s.maxdelay_h as f64 * roomsize) as i32;
        s.delay_i = (s.maxdelay_i as f64 * roomsize) as i32;
        s.delay_j = (s.maxdelay_j as f64 * roomsize) as i32;
        s.delay_k = (s.maxdelay_k as f64 * roomsize) as i32;
        s.delay_l = (s.maxdelay_l as f64 * roomsize) as i32;
        s.delay_m = (s.maxdelay_m as f64 * roomsize) as i32;
        s.delay_n = (s.maxdelay_n as f64 * roomsize) as i32;
        s.delay_o = (s.maxdelay_o as f64 * roomsize) as i32;
        s.delay_p = (s.maxdelay_p as f64 * roomsize) as i32;
        s.delay_q = (s.maxdelay_q as f64 * roomsize) as i32;
        s.delay_r = (s.maxdelay_r as f64 * roomsize) as i32;
        s.delay_s = (s.maxdelay_s as f64 * roomsize) as i32;
        s.delay_t = (s.maxdelay_t as f64 * roomsize) as i32;
        s.delay_u = (s.maxdelay_u as f64 * roomsize) as i32;
        s.delay_v = (s.maxdelay_v as f64 * roomsize) as i32;
        s.delay_w = (s.maxdelay_w as f64 * roomsize) as i32;
        s.delay_x = (s.maxdelay_x as f64 * roomsize) as i32;
        s.delay_y = (s.maxdelay_y as f64 * roomsize) as i32;
        s.delay_z = (s.maxdelay_z as f64 * roomsize) as i32;
        s.delay_mid = (s.maxdelay_mid as f64 * roomsize) as i32;
        s.delay_side = (s.maxdelay_side as f64 * roomsize) as i32;
        s.delay_left = (s.maxdelay_left as f64 * roomsize) as i32;
        s.delay_right = (s.maxdelay_right as f64 * roomsize) as i32;
        s.delaypre = (s.maxdelaypre as f64 * predelay) as i32;
        for smp in 0..l.len() {
            input_sample_l = l[smp] as f64;
            input_sample_r = r[smp] as f64;
            if input_sample_l.abs() < 1.18e-23 {
                input_sample_l = s.fpd_l as f64 * 1.18e-17;
            }
            if input_sample_r.abs() < 1.18e-23 {
                input_sample_r = s.fpd_r as f64 * 1.18e-17;
            }
            dry_sample_l = input_sample_l;
            dry_sample_r = input_sample_r;
            s.dpre_l[s.onepre as usize] = input_sample_l;
            s.dpre_r[s.onepre as usize] = input_sample_r;
            s.onepre -= 1;
            if s.onepre < 0 || s.onepre > s.delaypre {
                s.onepre = s.delaypre;
            }
            input_sample_l = s.dpre_l[s.onepre as usize];
            input_sample_r = s.dpre_r[s.onepre as usize];
            s.interpol_a += s.pitchshift_a * pspeed;
            s.interpol_b += s.pitchshift_b * pspeed;
            s.interpol_c += s.pitchshift_c * pspeed;
            s.interpol_d += s.pitchshift_d * pspeed;
            s.interpol_e += s.pitchshift_e * pspeed;
            s.interpol_f += s.pitchshift_f * pspeed;
            s.interpol_g += s.pitchshift_g * pspeed;
            s.interpol_h += s.pitchshift_h * pspeed;
            s.interpol_i += s.pitchshift_i * pspeed;
            s.interpol_j += s.pitchshift_j * pspeed;
            s.interpol_k += s.pitchshift_k * pspeed;
            s.interpol_l += s.pitchshift_l * pspeed;
            s.interpol_m += s.pitchshift_m * pspeed;
            s.interpol_n += s.pitchshift_n * pspeed;
            s.interpol_o += s.pitchshift_o * pspeed;
            s.interpol_p += s.pitchshift_p * pspeed;
            s.interpol_q += s.pitchshift_q * pspeed;
            s.interpol_r += s.pitchshift_r * pspeed;
            s.interpol_s += s.pitchshift_s * pspeed;
            s.interpol_t += s.pitchshift_t * pspeed;
            s.interpol_u += s.pitchshift_u * pspeed;
            s.interpol_v += s.pitchshift_v * pspeed;
            s.interpol_w += s.pitchshift_w * pspeed;
            s.interpol_x += s.pitchshift_x * pspeed;
            s.interpol_y += s.pitchshift_y * pspeed;
            s.interpol_z += s.pitchshift_z * pspeed;
            if s.interpol_a > 1.0 {
                s.pitchshift_a = -s.pitchshift_a.abs();
                s.interpol_a += s.pitchshift_a * pspeed;
            }
            if s.interpol_b > 1.0 {
                s.pitchshift_b = -s.pitchshift_b.abs();
                s.interpol_b += s.pitchshift_b * pspeed;
            }
            if s.interpol_c > 1.0 {
                s.pitchshift_c = -s.pitchshift_c.abs();
                s.interpol_c += s.pitchshift_c * pspeed;
            }
            if s.interpol_d > 1.0 {
                s.pitchshift_d = -s.pitchshift_d.abs();
                s.interpol_d += s.pitchshift_d * pspeed;
            }
            if s.interpol_e > 1.0 {
                s.pitchshift_e = -s.pitchshift_e.abs();
                s.interpol_e += s.pitchshift_e * pspeed;
            }
            if s.interpol_f > 1.0 {
                s.pitchshift_f = -s.pitchshift_f.abs();
                s.interpol_f += s.pitchshift_f * pspeed;
            }
            if s.interpol_g > 1.0 {
                s.pitchshift_g = -s.pitchshift_g.abs();
                s.interpol_g += s.pitchshift_g * pspeed;
            }
            if s.interpol_h > 1.0 {
                s.pitchshift_h = -s.pitchshift_h.abs();
                s.interpol_h += s.pitchshift_h * pspeed;
            }
            if s.interpol_i > 1.0 {
                s.pitchshift_i = -s.pitchshift_i.abs();
                s.interpol_i += s.pitchshift_i * pspeed;
            }
            if s.interpol_j > 1.0 {
                s.pitchshift_j = -s.pitchshift_j.abs();
                s.interpol_j += s.pitchshift_j * pspeed;
            }
            if s.interpol_k > 1.0 {
                s.pitchshift_k = -s.pitchshift_k.abs();
                s.interpol_k += s.pitchshift_k * pspeed;
            }
            if s.interpol_l > 1.0 {
                s.pitchshift_l = -s.pitchshift_l.abs();
                s.interpol_l += s.pitchshift_l * pspeed;
            }
            if s.interpol_m > 1.0 {
                s.pitchshift_m = -s.pitchshift_m.abs();
                s.interpol_m += s.pitchshift_m * pspeed;
            }
            if s.interpol_n > 1.0 {
                s.pitchshift_n = -s.pitchshift_n.abs();
                s.interpol_n += s.pitchshift_n * pspeed;
            }
            if s.interpol_o > 1.0 {
                s.pitchshift_o = -s.pitchshift_o.abs();
                s.interpol_o += s.pitchshift_o * pspeed;
            }
            if s.interpol_p > 1.0 {
                s.pitchshift_p = -s.pitchshift_p.abs();
                s.interpol_p += s.pitchshift_p * pspeed;
            }
            if s.interpol_q > 1.0 {
                s.pitchshift_q = -s.pitchshift_q.abs();
                s.interpol_q += s.pitchshift_q * pspeed;
            }
            if s.interpol_r > 1.0 {
                s.pitchshift_r = -s.pitchshift_r.abs();
                s.interpol_r += s.pitchshift_r * pspeed;
            }
            if s.interpol_s > 1.0 {
                s.pitchshift_s = -s.pitchshift_s.abs();
                s.interpol_s += s.pitchshift_s * pspeed;
            }
            if s.interpol_t > 1.0 {
                s.pitchshift_t = -s.pitchshift_t.abs();
                s.interpol_t += s.pitchshift_t * pspeed;
            }
            if s.interpol_u > 1.0 {
                s.pitchshift_u = -s.pitchshift_u.abs();
                s.interpol_u += s.pitchshift_u * pspeed;
            }
            if s.interpol_v > 1.0 {
                s.pitchshift_v = -s.pitchshift_v.abs();
                s.interpol_v += s.pitchshift_v * pspeed;
            }
            if s.interpol_w > 1.0 {
                s.pitchshift_w = -s.pitchshift_w.abs();
                s.interpol_w += s.pitchshift_w * pspeed;
            }
            if s.interpol_x > 1.0 {
                s.pitchshift_x = -s.pitchshift_x.abs();
                s.interpol_x += s.pitchshift_x * pspeed;
            }
            if s.interpol_y > 1.0 {
                s.pitchshift_y = -s.pitchshift_y.abs();
                s.interpol_y += s.pitchshift_y * pspeed;
            }
            if s.interpol_z > 1.0 {
                s.pitchshift_z = -s.pitchshift_z.abs();
                s.interpol_z += s.pitchshift_z * pspeed;
            }
            if s.interpol_a < 0.0 {
                s.pitchshift_a = s.pitchshift_a.abs();
                s.interpol_a += s.pitchshift_a * pspeed;
            }
            if s.interpol_b < 0.0 {
                s.pitchshift_b = s.pitchshift_b.abs();
                s.interpol_b += s.pitchshift_b * pspeed;
            }
            if s.interpol_c < 0.0 {
                s.pitchshift_c = s.pitchshift_c.abs();
                s.interpol_c += s.pitchshift_c * pspeed;
            }
            if s.interpol_d < 0.0 {
                s.pitchshift_d = s.pitchshift_d.abs();
                s.interpol_d += s.pitchshift_d * pspeed;
            }
            if s.interpol_e < 0.0 {
                s.pitchshift_e = s.pitchshift_e.abs();
                s.interpol_e += s.pitchshift_e * pspeed;
            }
            if s.interpol_f < 0.0 {
                s.pitchshift_f = s.pitchshift_f.abs();
                s.interpol_f += s.pitchshift_f * pspeed;
            }
            if s.interpol_g < 0.0 {
                s.pitchshift_g = s.pitchshift_g.abs();
                s.interpol_g += s.pitchshift_g * pspeed;
            }
            if s.interpol_h < 0.0 {
                s.pitchshift_h = s.pitchshift_h.abs();
                s.interpol_h += s.pitchshift_h * pspeed;
            }
            if s.interpol_i < 0.0 {
                s.pitchshift_i = s.pitchshift_i.abs();
                s.interpol_i += s.pitchshift_i * pspeed;
            }
            if s.interpol_j < 0.0 {
                s.pitchshift_j = s.pitchshift_j.abs();
                s.interpol_j += s.pitchshift_j * pspeed;
            }
            if s.interpol_k < 0.0 {
                s.pitchshift_k = s.pitchshift_k.abs();
                s.interpol_k += s.pitchshift_k * pspeed;
            }
            if s.interpol_l < 0.0 {
                s.pitchshift_l = s.pitchshift_l.abs();
                s.interpol_l += s.pitchshift_l * pspeed;
            }
            if s.interpol_m < 0.0 {
                s.pitchshift_m = s.pitchshift_m.abs();
                s.interpol_m += s.pitchshift_m * pspeed;
            }
            if s.interpol_n < 0.0 {
                s.pitchshift_n = s.pitchshift_n.abs();
                s.interpol_n += s.pitchshift_n * pspeed;
            }
            if s.interpol_o < 0.0 {
                s.pitchshift_o = s.pitchshift_o.abs();
                s.interpol_o += s.pitchshift_o * pspeed;
            }
            if s.interpol_p < 0.0 {
                s.pitchshift_p = s.pitchshift_p.abs();
                s.interpol_p += s.pitchshift_p * pspeed;
            }
            if s.interpol_q < 0.0 {
                s.pitchshift_q = s.pitchshift_q.abs();
                s.interpol_q += s.pitchshift_q * pspeed;
            }
            if s.interpol_r < 0.0 {
                s.pitchshift_r = s.pitchshift_r.abs();
                s.interpol_r += s.pitchshift_r * pspeed;
            }
            if s.interpol_s < 0.0 {
                s.pitchshift_s = s.pitchshift_s.abs();
                s.interpol_s += s.pitchshift_s * pspeed;
            }
            if s.interpol_t < 0.0 {
                s.pitchshift_t = s.pitchshift_t.abs();
                s.interpol_t += s.pitchshift_t * pspeed;
            }
            if s.interpol_u < 0.0 {
                s.pitchshift_u = s.pitchshift_u.abs();
                s.interpol_u += s.pitchshift_u * pspeed;
            }
            if s.interpol_v < 0.0 {
                s.pitchshift_v = s.pitchshift_v.abs();
                s.interpol_v += s.pitchshift_v * pspeed;
            }
            if s.interpol_w < 0.0 {
                s.pitchshift_w = s.pitchshift_w.abs();
                s.interpol_w += s.pitchshift_w * pspeed;
            }
            if s.interpol_x < 0.0 {
                s.pitchshift_x = s.pitchshift_x.abs();
                s.interpol_x += s.pitchshift_x * pspeed;
            }
            if s.interpol_y < 0.0 {
                s.pitchshift_y = s.pitchshift_y.abs();
                s.interpol_y += s.pitchshift_y * pspeed;
            }
            if s.interpol_z < 0.0 {
                s.pitchshift_z = s.pitchshift_z.abs();
                s.interpol_z += s.pitchshift_z * pspeed;
            }
            if s.verbout_r > 1.0 {
                s.verbout_r = 1.0;
            }
            if s.verbout_r < -1.0 {
                s.verbout_r = -1.0;
            }
            if s.verbout_l > 1.0 {
                s.verbout_l = 1.0;
            }
            if s.verbout_l < -1.0 {
                s.verbout_l = -1.0;
            }
            input_sample_l += s.verbout_r;
            input_sample_r += s.verbout_l;
            s.verbout_l = 0.0;
            s.verbout_r = 0.0;
            mid = input_sample_l + input_sample_r;
            side = input_sample_l - input_sample_r;
            allpasstemp = s.one_mid - 1;
            if allpasstemp < 0 || allpasstemp > s.delay_mid {
                allpasstemp = s.delay_mid;
            }
            mid -= s.d_mid[allpasstemp as usize] * constallpass;
            s.d_mid[s.one_mid as usize] = mid;
            mid *= constallpass;
            s.one_mid -= 1;
            if s.one_mid < 0 || s.one_mid > s.delay_mid {
                s.one_mid = s.delay_mid;
            }
            mid += s.d_mid[s.one_mid as usize];
            s.nonlin += s.d_mid[s.one_mid as usize].abs();
            allpasstemp = s.one_side - 1;
            if allpasstemp < 0 || allpasstemp > s.delay_side {
                allpasstemp = s.delay_side;
            }
            side -= s.d_side[allpasstemp as usize] * constallpass;
            s.d_side[s.one_side as usize] = side;
            side *= constallpass;
            s.one_side -= 1;
            if s.one_side < 0 || s.one_side > s.delay_side {
                s.one_side = s.delay_side;
            }
            side += s.d_side[s.one_side as usize];
            s.nonlin += s.d_side[s.one_side as usize].abs();
            allpasstemp = s.one_left - 1;
            if allpasstemp < 0 || allpasstemp > s.delay_left {
                allpasstemp = s.delay_left;
            }
            input_sample_l -= s.d_left[allpasstemp as usize] * constallpass;
            s.d_left[s.one_left as usize] = s.verbout_l;
            input_sample_l *= constallpass;
            s.one_left -= 1;
            if s.one_left < 0 || s.one_left > s.delay_left {
                s.one_left = s.delay_left;
            }
            input_sample_l += s.d_left[s.one_left as usize];
            s.nonlin += s.d_left[s.one_left as usize].abs();
            allpasstemp = s.one_right - 1;
            if allpasstemp < 0 || allpasstemp > s.delay_right {
                allpasstemp = s.delay_right;
            }
            input_sample_r -= s.d_right[allpasstemp as usize] * constallpass;
            s.d_right[s.one_right as usize] = s.verbout_r;
            input_sample_r *= constallpass;
            s.one_right -= 1;
            if s.one_right < 0 || s.one_right > s.delay_right {
                s.one_right = s.delay_right;
            }
            input_sample_r += s.d_right[s.one_right as usize];
            s.nonlin += s.d_right[s.one_right as usize].abs();
            input_sample_l += (mid + side) / 2.0;
            input_sample_r += (mid - side) / 2.0;
            temp = s.d_a[s.one_a as usize] * s.interpol_a;
            temp += s.d_a[s.tre_a as usize] * (1.0 - s.interpol_a);
            temp += s.d_a[s.two_a as usize];
            s.d_a[s.tre_a as usize] = temp * tankfeedback;
            s.d_a[s.tre_a as usize] += input_sample_l;
            s.one_a -= 1;
            if s.one_a < 0 || s.one_a > s.delay_a {
                s.one_a = s.delay_a;
            }
            s.two_a -= 1;
            if s.two_a < 0 || s.two_a > s.delay_a {
                s.two_a = s.delay_a;
            }
            s.tre_a -= 1;
            if s.tre_a < 0 || s.tre_a > s.delay_a {
                s.tre_a = s.delay_a;
            }
            temp = s.d_a[s.one_a as usize] * s.interpol_a;
            temp += s.d_a[s.tre_a as usize] * (1.0 - s.interpol_a);
            temp *= invlean + lean * s.d_a[s.two_a as usize].abs();
            s.verbout_l += temp;
            temp = s.d_c[s.one_c as usize] * s.interpol_c;
            temp += s.d_c[s.tre_c as usize] * (1.0 - s.interpol_c);
            temp += s.d_c[s.two_c as usize];
            s.d_c[s.tre_c as usize] = temp * tankfeedback;
            s.d_c[s.tre_c as usize] += input_sample_l;
            s.one_c -= 1;
            if s.one_c < 0 || s.one_c > s.delay_c {
                s.one_c = s.delay_c;
            }
            s.two_c -= 1;
            if s.two_c < 0 || s.two_c > s.delay_c {
                s.two_c = s.delay_c;
            }
            s.tre_c -= 1;
            if s.tre_c < 0 || s.tre_c > s.delay_c {
                s.tre_c = s.delay_c;
            }
            temp = s.d_c[s.one_c as usize] * s.interpol_c;
            temp += s.d_c[s.tre_c as usize] * (1.0 - s.interpol_c);
            temp *= invlean + lean * s.d_c[s.two_c as usize].abs();
            s.verbout_l += temp;
            temp = s.d_e[s.one_e as usize] * s.interpol_e;
            temp += s.d_e[s.tre_e as usize] * (1.0 - s.interpol_e);
            temp += s.d_e[s.two_e as usize];
            s.d_e[s.tre_e as usize] = temp * tankfeedback;
            s.d_e[s.tre_e as usize] += input_sample_l;
            s.one_e -= 1;
            if s.one_e < 0 || s.one_e > s.delay_e {
                s.one_e = s.delay_e;
            }
            s.two_e -= 1;
            if s.two_e < 0 || s.two_e > s.delay_e {
                s.two_e = s.delay_e;
            }
            s.tre_e -= 1;
            if s.tre_e < 0 || s.tre_e > s.delay_e {
                s.tre_e = s.delay_e;
            }
            temp = s.d_e[s.one_e as usize] * s.interpol_e;
            temp += s.d_e[s.tre_e as usize] * (1.0 - s.interpol_e);
            temp *= invlean + lean * s.d_e[s.two_e as usize].abs();
            s.verbout_l += temp;
            temp = s.d_g[s.one_g as usize] * s.interpol_g;
            temp += s.d_g[s.tre_g as usize] * (1.0 - s.interpol_g);
            temp += s.d_g[s.two_g as usize];
            s.d_g[s.tre_g as usize] = temp * tankfeedback;
            s.d_g[s.tre_g as usize] += input_sample_l;
            s.one_g -= 1;
            if s.one_g < 0 || s.one_g > s.delay_g {
                s.one_g = s.delay_g;
            }
            s.two_g -= 1;
            if s.two_g < 0 || s.two_g > s.delay_g {
                s.two_g = s.delay_g;
            }
            s.tre_g -= 1;
            if s.tre_g < 0 || s.tre_g > s.delay_g {
                s.tre_g = s.delay_g;
            }
            temp = s.d_g[s.one_g as usize] * s.interpol_g;
            temp += s.d_g[s.tre_g as usize] * (1.0 - s.interpol_g);
            temp *= invlean + lean * s.d_g[s.two_g as usize].abs();
            s.verbout_l += temp;
            temp = s.d_i[s.one_i as usize] * s.interpol_i;
            temp += s.d_i[s.tre_i as usize] * (1.0 - s.interpol_i);
            temp += s.d_i[s.two_i as usize];
            s.d_i[s.tre_i as usize] = temp * tankfeedback;
            s.d_i[s.tre_i as usize] += input_sample_l;
            s.one_i -= 1;
            if s.one_i < 0 || s.one_i > s.delay_i {
                s.one_i = s.delay_i;
            }
            s.two_i -= 1;
            if s.two_i < 0 || s.two_i > s.delay_i {
                s.two_i = s.delay_i;
            }
            s.tre_i -= 1;
            if s.tre_i < 0 || s.tre_i > s.delay_i {
                s.tre_i = s.delay_i;
            }
            temp = s.d_i[s.one_i as usize] * s.interpol_i;
            temp += s.d_i[s.tre_i as usize] * (1.0 - s.interpol_i);
            temp *= invlean + lean * s.d_i[s.two_i as usize].abs();
            s.verbout_l += temp;
            temp = s.d_k[s.one_k as usize] * s.interpol_k;
            temp += s.d_k[s.tre_k as usize] * (1.0 - s.interpol_k);
            temp += s.d_k[s.two_k as usize];
            s.d_k[s.tre_k as usize] = temp * tankfeedback;
            s.d_k[s.tre_k as usize] += input_sample_l;
            s.one_k -= 1;
            if s.one_k < 0 || s.one_k > s.delay_k {
                s.one_k = s.delay_k;
            }
            s.two_k -= 1;
            if s.two_k < 0 || s.two_k > s.delay_k {
                s.two_k = s.delay_k;
            }
            s.tre_k -= 1;
            if s.tre_k < 0 || s.tre_k > s.delay_k {
                s.tre_k = s.delay_k;
            }
            temp = s.d_k[s.one_k as usize] * s.interpol_k;
            temp += s.d_k[s.tre_k as usize] * (1.0 - s.interpol_k);
            temp *= invlean + lean * s.d_k[s.two_k as usize].abs();
            s.verbout_l += temp;
            temp = s.d_m[s.one_m as usize] * s.interpol_m;
            temp += s.d_m[s.tre_m as usize] * (1.0 - s.interpol_m);
            temp += s.d_m[s.two_m as usize];
            s.d_m[s.tre_m as usize] = temp * tankfeedback;
            s.d_m[s.tre_m as usize] += input_sample_l;
            s.one_m -= 1;
            if s.one_m < 0 || s.one_m > s.delay_m {
                s.one_m = s.delay_m;
            }
            s.two_m -= 1;
            if s.two_m < 0 || s.two_m > s.delay_m {
                s.two_m = s.delay_m;
            }
            s.tre_m -= 1;
            if s.tre_m < 0 || s.tre_m > s.delay_m {
                s.tre_m = s.delay_m;
            }
            temp = s.d_m[s.one_m as usize] * s.interpol_m;
            temp += s.d_m[s.tre_m as usize] * (1.0 - s.interpol_m);
            temp *= invlean + lean * s.d_m[s.two_m as usize].abs();
            s.verbout_l += temp;
            temp = s.d_o[s.one_o as usize] * s.interpol_o;
            temp += s.d_o[s.tre_o as usize] * (1.0 - s.interpol_o);
            temp += s.d_o[s.two_o as usize];
            s.d_o[s.tre_o as usize] = temp * tankfeedback;
            s.d_o[s.tre_o as usize] += input_sample_l;
            s.one_o -= 1;
            if s.one_o < 0 || s.one_o > s.delay_o {
                s.one_o = s.delay_o;
            }
            s.two_o -= 1;
            if s.two_o < 0 || s.two_o > s.delay_o {
                s.two_o = s.delay_o;
            }
            s.tre_o -= 1;
            if s.tre_o < 0 || s.tre_o > s.delay_o {
                s.tre_o = s.delay_o;
            }
            temp = s.d_o[s.one_o as usize] * s.interpol_o;
            temp += s.d_o[s.tre_o as usize] * (1.0 - s.interpol_o);
            temp *= invlean + lean * s.d_o[s.two_o as usize].abs();
            s.verbout_l += temp;
            temp = s.d_q[s.one_q as usize] * s.interpol_q;
            temp += s.d_q[s.tre_q as usize] * (1.0 - s.interpol_q);
            temp += s.d_q[s.two_q as usize];
            s.d_q[s.tre_q as usize] = temp * tankfeedback;
            s.d_q[s.tre_q as usize] += input_sample_l;
            s.one_q -= 1;
            if s.one_q < 0 || s.one_q > s.delay_q {
                s.one_q = s.delay_q;
            }
            s.two_q -= 1;
            if s.two_q < 0 || s.two_q > s.delay_q {
                s.two_q = s.delay_q;
            }
            s.tre_q -= 1;
            if s.tre_q < 0 || s.tre_q > s.delay_q {
                s.tre_q = s.delay_q;
            }
            temp = s.d_q[s.one_q as usize] * s.interpol_q;
            temp += s.d_q[s.tre_q as usize] * (1.0 - s.interpol_q);
            temp *= invlean + lean * s.d_q[s.two_q as usize].abs();
            s.verbout_l += temp;
            temp = s.d_s[s.one_s as usize] * s.interpol_s;
            temp += s.d_s[s.tre_s as usize] * (1.0 - s.interpol_s);
            temp += s.d_s[s.two_s as usize];
            s.d_s[s.tre_s as usize] = temp * tankfeedback;
            s.d_s[s.tre_s as usize] += input_sample_l;
            s.one_s -= 1;
            if s.one_s < 0 || s.one_s > s.delay_s {
                s.one_s = s.delay_s;
            }
            s.two_s -= 1;
            if s.two_s < 0 || s.two_s > s.delay_s {
                s.two_s = s.delay_s;
            }
            s.tre_s -= 1;
            if s.tre_s < 0 || s.tre_s > s.delay_s {
                s.tre_s = s.delay_s;
            }
            temp = s.d_s[s.one_s as usize] * s.interpol_s;
            temp += s.d_s[s.tre_s as usize] * (1.0 - s.interpol_s);
            temp *= invlean + lean * s.d_s[s.two_s as usize].abs();
            s.verbout_l += temp;
            temp = s.d_u[s.one_u as usize] * s.interpol_u;
            temp += s.d_u[s.tre_u as usize] * (1.0 - s.interpol_u);
            temp += s.d_u[s.two_u as usize];
            s.d_u[s.tre_u as usize] = temp * tankfeedback;
            s.d_u[s.tre_u as usize] += input_sample_l;
            s.one_u -= 1;
            if s.one_u < 0 || s.one_u > s.delay_u {
                s.one_u = s.delay_u;
            }
            s.two_u -= 1;
            if s.two_u < 0 || s.two_u > s.delay_u {
                s.two_u = s.delay_u;
            }
            s.tre_u -= 1;
            if s.tre_u < 0 || s.tre_u > s.delay_u {
                s.tre_u = s.delay_u;
            }
            temp = s.d_u[s.one_u as usize] * s.interpol_u;
            temp += s.d_u[s.tre_u as usize] * (1.0 - s.interpol_u);
            temp *= invlean + lean * s.d_u[s.two_u as usize].abs();
            s.verbout_l += temp;
            temp = s.d_w[s.one_w as usize] * s.interpol_w;
            temp += s.d_w[s.tre_w as usize] * (1.0 - s.interpol_w);
            temp += s.d_w[s.two_w as usize];
            s.d_w[s.tre_w as usize] = temp * tankfeedback;
            s.d_w[s.tre_w as usize] += input_sample_l;
            s.one_w -= 1;
            if s.one_w < 0 || s.one_w > s.delay_w {
                s.one_w = s.delay_w;
            }
            s.two_w -= 1;
            if s.two_w < 0 || s.two_w > s.delay_w {
                s.two_w = s.delay_w;
            }
            s.tre_w -= 1;
            if s.tre_w < 0 || s.tre_w > s.delay_w {
                s.tre_w = s.delay_w;
            }
            temp = s.d_w[s.one_w as usize] * s.interpol_w;
            temp += s.d_w[s.tre_w as usize] * (1.0 - s.interpol_w);
            temp *= invlean + lean * s.d_w[s.two_w as usize].abs();
            s.verbout_l += temp;
            temp = s.d_y[s.one_y as usize] * s.interpol_y;
            temp += s.d_y[s.tre_y as usize] * (1.0 - s.interpol_y);
            temp += s.d_y[s.two_y as usize];
            s.d_y[s.tre_y as usize] = temp * tankfeedback;
            s.d_y[s.tre_y as usize] += input_sample_l;
            s.one_y -= 1;
            if s.one_y < 0 || s.one_y > s.delay_y {
                s.one_y = s.delay_y;
            }
            s.two_y -= 1;
            if s.two_y < 0 || s.two_y > s.delay_y {
                s.two_y = s.delay_y;
            }
            s.tre_y -= 1;
            if s.tre_y < 0 || s.tre_y > s.delay_y {
                s.tre_y = s.delay_y;
            }
            temp = s.d_y[s.one_y as usize] * s.interpol_y;
            temp += s.d_y[s.tre_y as usize] * (1.0 - s.interpol_y);
            temp *= invlean + lean * s.d_y[s.two_y as usize].abs();
            s.verbout_l += temp;
            temp = s.d_b[s.one_b as usize] * s.interpol_b;
            temp += s.d_b[s.tre_b as usize] * (1.0 - s.interpol_b);
            temp += s.d_b[s.two_b as usize];
            s.d_b[s.tre_b as usize] = temp * tankfeedback;
            s.d_b[s.tre_b as usize] += input_sample_r;
            s.one_b -= 1;
            if s.one_b < 0 || s.one_b > s.delay_b {
                s.one_b = s.delay_b;
            }
            s.two_b -= 1;
            if s.two_b < 0 || s.two_b > s.delay_b {
                s.two_b = s.delay_b;
            }
            s.tre_b -= 1;
            if s.tre_b < 0 || s.tre_b > s.delay_b {
                s.tre_b = s.delay_b;
            }
            temp = s.d_b[s.one_b as usize] * s.interpol_b;
            temp += s.d_b[s.tre_b as usize] * (1.0 - s.interpol_b);
            temp *= invlean + lean * s.d_b[s.two_b as usize].abs();
            s.verbout_r += temp;
            temp = s.d_d[s.one_d as usize] * s.interpol_d;
            temp += s.d_d[s.tre_d as usize] * (1.0 - s.interpol_d);
            temp += s.d_d[s.two_d as usize];
            s.d_d[s.tre_d as usize] = temp * tankfeedback;
            s.d_d[s.tre_d as usize] += input_sample_r;
            s.one_d -= 1;
            if s.one_d < 0 || s.one_d > s.delay_d {
                s.one_d = s.delay_d;
            }
            s.two_d -= 1;
            if s.two_d < 0 || s.two_d > s.delay_d {
                s.two_d = s.delay_d;
            }
            s.tre_d -= 1;
            if s.tre_d < 0 || s.tre_d > s.delay_d {
                s.tre_d = s.delay_d;
            }
            temp = s.d_d[s.one_d as usize] * s.interpol_d;
            temp += s.d_d[s.tre_d as usize] * (1.0 - s.interpol_d);
            temp *= invlean + lean * s.d_d[s.two_d as usize].abs();
            s.verbout_r += temp;
            temp = s.d_f[s.one_f as usize] * s.interpol_f;
            temp += s.d_f[s.tre_f as usize] * (1.0 - s.interpol_f);
            temp += s.d_f[s.two_f as usize];
            s.d_f[s.tre_f as usize] = temp * tankfeedback;
            s.d_f[s.tre_f as usize] += input_sample_r;
            s.one_f -= 1;
            if s.one_f < 0 || s.one_f > s.delay_f {
                s.one_f = s.delay_f;
            }
            s.two_f -= 1;
            if s.two_f < 0 || s.two_f > s.delay_f {
                s.two_f = s.delay_f;
            }
            s.tre_f -= 1;
            if s.tre_f < 0 || s.tre_f > s.delay_f {
                s.tre_f = s.delay_f;
            }
            temp = s.d_f[s.one_f as usize] * s.interpol_f;
            temp += s.d_f[s.tre_f as usize] * (1.0 - s.interpol_f);
            temp *= invlean + lean * s.d_f[s.two_f as usize].abs();
            s.verbout_r += temp;
            temp = s.d_h[s.one_h as usize] * s.interpol_h;
            temp += s.d_h[s.tre_h as usize] * (1.0 - s.interpol_h);
            temp += s.d_h[s.two_h as usize];
            s.d_h[s.tre_h as usize] = temp * tankfeedback;
            s.d_h[s.tre_h as usize] += input_sample_r;
            s.one_h -= 1;
            if s.one_h < 0 || s.one_h > s.delay_h {
                s.one_h = s.delay_h;
            }
            s.two_h -= 1;
            if s.two_h < 0 || s.two_h > s.delay_h {
                s.two_h = s.delay_h;
            }
            s.tre_h -= 1;
            if s.tre_h < 0 || s.tre_h > s.delay_h {
                s.tre_h = s.delay_h;
            }
            temp = s.d_h[s.one_h as usize] * s.interpol_h;
            temp += s.d_h[s.tre_h as usize] * (1.0 - s.interpol_h);
            temp *= invlean + lean * s.d_h[s.two_h as usize].abs();
            s.verbout_r += temp;
            temp = s.d_j[s.one_j as usize] * s.interpol_j;
            temp += s.d_j[s.tre_j as usize] * (1.0 - s.interpol_j);
            temp += s.d_j[s.two_j as usize];
            s.d_j[s.tre_j as usize] = temp * tankfeedback;
            s.d_j[s.tre_j as usize] += input_sample_r;
            s.one_j -= 1;
            if s.one_j < 0 || s.one_j > s.delay_j {
                s.one_j = s.delay_j;
            }
            s.two_j -= 1;
            if s.two_j < 0 || s.two_j > s.delay_j {
                s.two_j = s.delay_j;
            }
            s.tre_j -= 1;
            if s.tre_j < 0 || s.tre_j > s.delay_j {
                s.tre_j = s.delay_j;
            }
            temp = s.d_j[s.one_j as usize] * s.interpol_j;
            temp += s.d_j[s.tre_j as usize] * (1.0 - s.interpol_j);
            temp *= invlean + lean * s.d_j[s.two_j as usize].abs();
            s.verbout_r += temp;
            temp = s.d_l[s.one_l as usize] * s.interpol_l;
            temp += s.d_l[s.tre_l as usize] * (1.0 - s.interpol_l);
            temp += s.d_l[s.two_l as usize];
            s.d_l[s.tre_l as usize] = temp * tankfeedback;
            s.d_l[s.tre_l as usize] += input_sample_r;
            s.one_l -= 1;
            if s.one_l < 0 || s.one_l > s.delay_l {
                s.one_l = s.delay_l;
            }
            s.two_l -= 1;
            if s.two_l < 0 || s.two_l > s.delay_l {
                s.two_l = s.delay_l;
            }
            s.tre_l -= 1;
            if s.tre_l < 0 || s.tre_l > s.delay_l {
                s.tre_l = s.delay_l;
            }
            temp = s.d_l[s.one_l as usize] * s.interpol_l;
            temp += s.d_l[s.tre_l as usize] * (1.0 - s.interpol_l);
            temp *= invlean + lean * s.d_l[s.two_l as usize].abs();
            s.verbout_r += temp;
            temp = s.d_n[s.one_n as usize] * s.interpol_n;
            temp += s.d_n[s.tre_n as usize] * (1.0 - s.interpol_n);
            temp += s.d_n[s.two_n as usize];
            s.d_n[s.tre_n as usize] = temp * tankfeedback;
            s.d_n[s.tre_n as usize] += input_sample_r;
            s.one_n -= 1;
            if s.one_n < 0 || s.one_n > s.delay_n {
                s.one_n = s.delay_n;
            }
            s.two_n -= 1;
            if s.two_n < 0 || s.two_n > s.delay_n {
                s.two_n = s.delay_n;
            }
            s.tre_n -= 1;
            if s.tre_n < 0 || s.tre_n > s.delay_n {
                s.tre_n = s.delay_n;
            }
            temp = s.d_n[s.one_n as usize] * s.interpol_n;
            temp += s.d_n[s.tre_n as usize] * (1.0 - s.interpol_n);
            temp *= invlean + lean * s.d_n[s.two_n as usize].abs();
            s.verbout_r += temp;
            temp = s.d_p[s.one_p as usize] * s.interpol_p;
            temp += s.d_p[s.tre_p as usize] * (1.0 - s.interpol_p);
            temp += s.d_p[s.two_p as usize];
            s.d_p[s.tre_p as usize] = temp * tankfeedback;
            s.d_p[s.tre_p as usize] += input_sample_r;
            s.one_p -= 1;
            if s.one_p < 0 || s.one_p > s.delay_p {
                s.one_p = s.delay_p;
            }
            s.two_p -= 1;
            if s.two_p < 0 || s.two_p > s.delay_p {
                s.two_p = s.delay_p;
            }
            s.tre_p -= 1;
            if s.tre_p < 0 || s.tre_p > s.delay_p {
                s.tre_p = s.delay_p;
            }
            temp = s.d_p[s.one_p as usize] * s.interpol_p;
            temp += s.d_p[s.tre_p as usize] * (1.0 - s.interpol_p);
            temp *= invlean + lean * s.d_p[s.two_p as usize].abs();
            s.verbout_r += temp;
            temp = s.d_r[s.one_r as usize] * s.interpol_r;
            temp += s.d_r[s.tre_r as usize] * (1.0 - s.interpol_r);
            temp += s.d_r[s.two_r as usize];
            s.d_r[s.tre_r as usize] = temp * tankfeedback;
            s.d_r[s.tre_r as usize] += input_sample_r;
            s.one_r -= 1;
            if s.one_r < 0 || s.one_r > s.delay_r {
                s.one_r = s.delay_r;
            }
            s.two_r -= 1;
            if s.two_r < 0 || s.two_r > s.delay_r {
                s.two_r = s.delay_r;
            }
            s.tre_r -= 1;
            if s.tre_r < 0 || s.tre_r > s.delay_r {
                s.tre_r = s.delay_r;
            }
            temp = s.d_r[s.one_r as usize] * s.interpol_r;
            temp += s.d_r[s.tre_r as usize] * (1.0 - s.interpol_r);
            temp *= invlean + lean * s.d_r[s.two_r as usize].abs();
            s.verbout_r += temp;
            temp = s.d_t[s.one_t as usize] * s.interpol_t;
            temp += s.d_t[s.tre_t as usize] * (1.0 - s.interpol_t);
            temp += s.d_t[s.two_t as usize];
            s.d_t[s.tre_t as usize] = temp * tankfeedback;
            s.d_t[s.tre_t as usize] += input_sample_r;
            s.one_t -= 1;
            if s.one_t < 0 || s.one_t > s.delay_t {
                s.one_t = s.delay_t;
            }
            s.two_t -= 1;
            if s.two_t < 0 || s.two_t > s.delay_t {
                s.two_t = s.delay_t;
            }
            s.tre_t -= 1;
            if s.tre_t < 0 || s.tre_t > s.delay_t {
                s.tre_t = s.delay_t;
            }
            temp = s.d_t[s.one_t as usize] * s.interpol_t;
            temp += s.d_t[s.tre_t as usize] * (1.0 - s.interpol_t);
            temp *= invlean + lean * s.d_t[s.two_t as usize].abs();
            s.verbout_r += temp;
            temp = s.d_v[s.one_v as usize] * s.interpol_v;
            temp += s.d_v[s.tre_v as usize] * (1.0 - s.interpol_v);
            temp += s.d_v[s.two_v as usize];
            s.d_v[s.tre_v as usize] = temp * tankfeedback;
            s.d_v[s.tre_v as usize] += input_sample_r;
            s.one_v -= 1;
            if s.one_v < 0 || s.one_v > s.delay_v {
                s.one_v = s.delay_v;
            }
            s.two_v -= 1;
            if s.two_v < 0 || s.two_v > s.delay_v {
                s.two_v = s.delay_v;
            }
            s.tre_v -= 1;
            if s.tre_v < 0 || s.tre_v > s.delay_v {
                s.tre_v = s.delay_v;
            }
            temp = s.d_v[s.one_v as usize] * s.interpol_v;
            temp += s.d_v[s.tre_v as usize] * (1.0 - s.interpol_v);
            temp *= invlean + lean * s.d_v[s.two_v as usize].abs();
            s.verbout_r += temp;
            temp = s.d_x[s.one_x as usize] * s.interpol_x;
            temp += s.d_x[s.tre_x as usize] * (1.0 - s.interpol_x);
            temp += s.d_x[s.two_x as usize];
            s.d_x[s.tre_x as usize] = temp * tankfeedback;
            s.d_x[s.tre_x as usize] += input_sample_r;
            s.one_x -= 1;
            if s.one_x < 0 || s.one_x > s.delay_x {
                s.one_x = s.delay_x;
            }
            s.two_x -= 1;
            if s.two_x < 0 || s.two_x > s.delay_x {
                s.two_x = s.delay_x;
            }
            s.tre_x -= 1;
            if s.tre_x < 0 || s.tre_x > s.delay_x {
                s.tre_x = s.delay_x;
            }
            temp = s.d_x[s.one_x as usize] * s.interpol_x;
            temp += s.d_x[s.tre_x as usize] * (1.0 - s.interpol_x);
            temp *= invlean + lean * s.d_x[s.two_x as usize].abs();
            s.verbout_r += temp;
            temp = s.d_z[s.one_z as usize] * s.interpol_z;
            temp += s.d_z[s.tre_z as usize] * (1.0 - s.interpol_z);
            temp += s.d_z[s.two_z as usize];
            s.d_z[s.tre_z as usize] = temp * tankfeedback;
            s.d_z[s.tre_z as usize] += input_sample_r;
            s.one_z -= 1;
            if s.one_z < 0 || s.one_z > s.delay_z {
                s.one_z = s.delay_z;
            }
            s.two_z -= 1;
            if s.two_z < 0 || s.two_z > s.delay_z {
                s.two_z = s.delay_z;
            }
            s.tre_z -= 1;
            if s.tre_z < 0 || s.tre_z > s.delay_z {
                s.tre_z = s.delay_z;
            }
            temp = s.d_z[s.one_z as usize] * s.interpol_z;
            temp += s.d_z[s.tre_z as usize] * (1.0 - s.interpol_z);
            temp *= invlean + lean * s.d_z[s.two_z as usize].abs();
            s.verbout_r += temp;
            s.verbout_l /= 8.0;
            s.verbout_r /= 8.0;
            s.iir_sample_l = s.iir_sample_l * (1.0 - iir_amount) + s.verbout_l * iir_amount;
            s.verbout_l = s.verbout_l - s.iir_sample_l;
            s.iir_sample_r = s.iir_sample_r * (1.0 - iir_amount) + s.verbout_r * iir_amount;
            s.verbout_r = s.verbout_r - s.iir_sample_r;
            s.verbout_l *= invlean + lean * s.verbout_l.abs();
            s.verbout_r *= invlean + lean * s.verbout_r.abs();
            input_sample_l = s.verbout_l;
            input_sample_r = s.verbout_r;
            if s.flip {
                s.lowpass_sample_aa = s.lowpass_sample_aa * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_aa;
                s.lowpass_sample_ba = s.lowpass_sample_ba * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_ba;
                s.lowpass_sample_ca = s.lowpass_sample_ca * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_ca;
                s.lowpass_sample_da = s.lowpass_sample_da * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_da;
                s.lowpass_sample_e = s.lowpass_sample_e * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_e;
            } else {
                s.lowpass_sample_ab = s.lowpass_sample_ab * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_ab;
                s.lowpass_sample_bb = s.lowpass_sample_bb * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_bb;
                s.lowpass_sample_cb = s.lowpass_sample_cb * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_cb;
                s.lowpass_sample_db = s.lowpass_sample_db * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_db;
                s.lowpass_sample_f = s.lowpass_sample_f * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
                input_sample_l = s.lowpass_sample_f;
            }
            s.lowpass_sample_g = s.lowpass_sample_g * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
            input_sample_l = s.lowpass_sample_g * (1.0 - iir_amount_c) + input_sample_l * iir_amount_c;
            if s.flip {
                s.rowpass_sample_aa = s.rowpass_sample_aa * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_aa;
                s.rowpass_sample_ba = s.rowpass_sample_ba * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_ba;
                s.rowpass_sample_ca = s.rowpass_sample_ca * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_ca;
                s.rowpass_sample_da = s.rowpass_sample_da * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_da;
                s.rowpass_sample_e = s.rowpass_sample_e * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_e;
            } else {
                s.rowpass_sample_ab = s.rowpass_sample_ab * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_ab;
                s.rowpass_sample_bb = s.rowpass_sample_bb * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_bb;
                s.rowpass_sample_cb = s.rowpass_sample_cb * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_cb;
                s.rowpass_sample_db = s.rowpass_sample_db * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_db;
                s.rowpass_sample_f = s.rowpass_sample_f * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
                input_sample_r = s.rowpass_sample_f;
            }
            s.rowpass_sample_g = s.rowpass_sample_g * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
            input_sample_r = s.rowpass_sample_g * (1.0 - iir_amount_c) + input_sample_r * iir_amount_c;
            s.iir_ccsample_l = s.iir_ccsample_l * (1.0 - iir_amount) + s.verbout_l * iir_amount;
            s.verbout_l = s.verbout_l - s.iir_ccsample_l;
            s.iir_ccsample_r = s.iir_ccsample_r * (1.0 - iir_amount) + s.verbout_r * iir_amount;
            s.verbout_r = s.verbout_r - s.iir_ccsample_r;
            s.verbout_l *= invlean + lean * s.verbout_l.abs();
            s.verbout_r *= invlean + lean * s.verbout_r.abs();
            s.verbout_l = input_sample_l * outcouple;
            s.verbout_r = input_sample_r * outcouple;
            s.nonlin += s.verbout_l.abs();
            s.nonlin += s.verbout_r.abs();
            dry_sample_l *= dryness;
            dry_sample_r *= dryness;
            input_sample_l *= wetness;
            input_sample_r *= wetness;
            input_sample_l += dry_sample_l;
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
            s.flip = !s.flip;
            l[smp] = input_sample_l as f32;
            r[smp] = input_sample_r as f32;
        }
    }
}
