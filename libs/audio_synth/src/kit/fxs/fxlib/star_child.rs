// Ported from StarChild (StarChildProc.cpp, StarChild.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Star Child (sub-effect id 25): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Sustain", 1.0),
    pf("Grain", 0.7),
    pf("Dry/Wet", 0.2),
];

#[allow(dead_code)]
pub struct StarChild {
    p: [f32; 3],
    sr: f32,
    fpd_l: u32,
    fpd_r: u32,
    d: Vec<f64>,
    d_count: i32,
    p_v: Vec<i32>,
    t: Vec<i32>,
    out_l: Vec<f64>,
    out_r: Vec<f64>,
    pitch_counter: i32,
    increment: i32,
    duty_cycle: i32,
    wear_l: [f64; 11],
    wear_r: [f64; 11],
    factor: [f64; 11],
    wear_lprev: f64,
    wear_rprev: f64,
}

impl StarChild {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<StarChild> {
        let mut s = Box::new(StarChild {
            p: [1.0, 0.7, 0.2],
            sr: sr as f32,
            fpd_l: 0,
            fpd_r: 0,
            d: vec![0.0; 45102],
            d_count: 0,
            p_v: vec![0; 171],
            t: vec![0; 171],
            out_l: vec![0.0; 171],
            out_r: vec![0.0; 171],
            pitch_counter: 0,
            increment: 0,
            duty_cycle: 0,
            wear_l: [0.0; 11],
            wear_r: [0.0; 11],
            factor: [0.0; 11],
            wear_lprev: 0.0,
            wear_rprev: 0.0,
        });
        let mut count: i32 = 0;
        s.p_v[0] = 1;
        s.p_v[1] = 11;
        s.p_v[2] = 13;
        s.p_v[3] = 17;
        s.p_v[4] = 19;
        s.p_v[5] = 23;
        s.p_v[6] = 29;
        s.p_v[7] = 31;
        s.p_v[8] = 37;
        s.p_v[9] = 41;
        s.p_v[10] = 43;
        s.p_v[11] = 47;
        s.p_v[12] = 53;
        s.p_v[13] = 59;
        s.p_v[14] = 61;
        s.p_v[15] = 67;
        s.p_v[16] = 71;
        s.p_v[17] = 73;
        s.p_v[18] = 79;
        s.p_v[19] = 83;
        s.p_v[20] = 89;
        s.p_v[21] = 97;
        s.p_v[22] = 101;
        s.p_v[23] = 103;
        s.p_v[24] = 107;
        s.p_v[25] = 109;
        s.p_v[26] = 113;
        s.p_v[27] = 127;
        s.p_v[28] = 131;
        s.p_v[29] = 137;
        s.p_v[30] = 139;
        s.p_v[31] = 149;
        s.p_v[32] = 151;
        s.p_v[33] = 157;
        s.p_v[34] = 163;
        s.p_v[35] = 167;
        s.p_v[36] = 173;
        s.p_v[37] = 179;
        s.p_v[38] = 181;
        s.p_v[39] = 191;
        s.p_v[40] = 193;
        s.p_v[41] = 197;
        s.p_v[42] = 199;
        s.p_v[43] = 211;
        s.p_v[44] = 223;
        s.p_v[45] = 227;
        s.p_v[46] = 229;
        s.p_v[47] = 233;
        s.p_v[48] = 239;
        s.p_v[49] = 241;
        s.p_v[50] = 251;
        s.p_v[51] = 257;
        s.p_v[52] = 263;
        s.p_v[53] = 269;
        s.p_v[54] = 271;
        s.p_v[55] = 277;
        s.p_v[56] = 281;
        s.p_v[57] = 283;
        s.p_v[58] = 293;
        s.p_v[59] = 307;
        s.p_v[60] = 311;
        s.p_v[61] = 313;
        s.p_v[62] = 317;
        s.p_v[63] = 331;
        s.p_v[64] = 337;
        s.p_v[65] = 347;
        s.p_v[66] = 349;
        s.p_v[67] = 353;
        s.p_v[68] = 359;
        s.p_v[69] = 367;
        s.p_v[70] = 373;
        s.p_v[71] = 379;
        s.p_v[72] = 383;
        s.p_v[73] = 389;
        s.p_v[74] = 397;
        s.p_v[75] = 401;
        s.p_v[76] = 409;
        s.p_v[77] = 419;
        s.p_v[78] = 421;
        s.p_v[79] = 431;
        s.p_v[80] = 433;
        s.p_v[81] = 439;
        s.p_v[82] = 443;
        s.p_v[83] = 449;
        s.p_v[84] = 457;
        s.p_v[85] = 461;
        s.p_v[86] = 463;
        s.p_v[87] = 467;
        s.p_v[88] = 479;
        s.p_v[89] = 487;
        s.p_v[90] = 491;
        s.p_v[91] = 499;
        s.p_v[92] = 503;
        s.p_v[93] = 509;
        s.p_v[94] = 521;
        s.p_v[95] = 523;
        s.p_v[96] = 541;
        s.p_v[97] = 547;
        s.p_v[98] = 557;
        s.p_v[99] = 563;
        s.p_v[100] = 569;
        s.p_v[101] = 571;
        s.p_v[102] = 577;
        s.p_v[103] = 587;
        s.p_v[104] = 593;
        s.p_v[105] = 599;
        s.p_v[106] = 601;
        s.p_v[107] = 607;
        s.p_v[108] = 613;
        s.p_v[109] = 617;
        s.p_v[110] = 619;
        s.p_v[111] = 631;
        s.p_v[112] = 641;
        s.p_v[113] = 643;
        s.p_v[114] = 647;
        s.p_v[115] = 653;
        s.p_v[116] = 659;
        s.p_v[117] = 661;
        s.p_v[118] = 673;
        s.p_v[119] = 677;
        s.p_v[120] = 683;
        s.p_v[121] = 691;
        s.p_v[122] = 701;
        s.p_v[123] = 709;
        s.p_v[124] = 719;
        s.p_v[125] = 727;
        s.p_v[126] = 733;
        s.p_v[127] = 739;
        s.p_v[128] = 743;
        s.p_v[129] = 751;
        s.p_v[130] = 757;
        s.p_v[131] = 761;
        s.p_v[132] = 769;
        s.p_v[133] = 773;
        s.p_v[134] = 787;
        s.p_v[135] = 797;
        s.p_v[136] = 809;
        s.p_v[137] = 811;
        s.p_v[138] = 821;
        s.p_v[139] = 823;
        s.p_v[140] = 827;
        s.p_v[141] = 829;
        s.p_v[142] = 839;
        s.p_v[143] = 853;
        s.p_v[144] = 857;
        s.p_v[145] = 859;
        s.p_v[146] = 863;
        s.p_v[147] = 877;
        s.p_v[148] = 881;
        s.p_v[149] = 883;
        s.p_v[150] = 887;
        s.p_v[151] = 907;
        s.p_v[152] = 911;
        s.p_v[153] = 919;
        s.p_v[154] = 929;
        s.p_v[155] = 937;
        s.p_v[156] = 941;
        s.p_v[157] = 947;
        s.p_v[158] = 953;
        s.p_v[159] = 967;
        s.p_v[160] = 971;
        s.p_v[161] = 977;
        s.p_v[162] = 983;
        s.p_v[163] = 991;
        s.p_v[164] = 997;
        s.p_v[165] = 998;
        s.p_v[166] = 999;
        let mut assign: i32 = 0;
        {
            count = 0;
            while count < 165 {
                s.t[count as usize] = s.p_v[count as usize];
                assign = s.p_v[count as usize] % 10;
                match assign {
                    1 => {
                        s.out_l[count as usize] = 0.0;
                        s.out_r[count as usize] = s.p_v[count as usize] as f64;
                    }
                    3 => {
                        s.out_l[count as usize] = s.p_v[count as usize] as f64 * 0.25;
                        s.out_r[count as usize] = s.p_v[count as usize] as f64 * 0.75;
                    }
                    7 => {
                        s.out_l[count as usize] = s.p_v[count as usize] as f64 * 0.75;
                        s.out_r[count as usize] = s.p_v[count as usize] as f64 * 0.25;
                    }
                    9 => {
                        s.out_l[count as usize] = s.p_v[count as usize] as f64;
                        s.out_r[count as usize] = 0.0;
                    }
                    _ => {}
                }
                count += 1;
            }
        }
        s.pitch_counter = 2;
        s.increment = 1;
        s.duty_cycle = 1;
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for StarChild {
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
        let mut buffer_l: i32 = 0;
        let mut buffer_r: i32 = 0;
        let mut range_direct: i32 = ((s.p[1] as f64).powf(2.0) * 156.0 + 7.0) as i32;
        let mut scale_direct: f32 = ((s.p[0] as f64).powf(2.0) * (3280.0 / range_direct as f64) + 2.0) as f32;
        let mut output_pad: f32 = ((4 * range_direct) as f64 * (range_direct as f64).sqrt()) as f32;
        let mut overallscale: f32 = ((1.0 - s.p[1] as f64) * 9.0 + 1.0) as f32;
        let mut gain: f32 = overallscale;
        if gain as f64 > 1.0 {
            s.factor[0] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[0] = gain as f64;
            gain = 0.0;
        }
        if gain as f64 > 1.0 {
            s.factor[1] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[1] = gain as f64;
            gain = 0.0;
        }
        if gain as f64 > 1.0 {
            s.factor[2] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[2] = gain as f64;
            gain = 0.0;
        }
        if gain as f64 > 1.0 {
            s.factor[3] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[3] = gain as f64;
            gain = 0.0;
        }
        if gain as f64 > 1.0 {
            s.factor[4] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[4] = gain as f64;
            gain = 0.0;
        }
        if gain as f64 > 1.0 {
            s.factor[5] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[5] = gain as f64;
            gain = 0.0;
        }
        if gain as f64 > 1.0 {
            s.factor[6] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[6] = gain as f64;
            gain = 0.0;
        }
        if gain as f64 > 1.0 {
            s.factor[7] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[7] = gain as f64;
            gain = 0.0;
        }
        if gain as f64 > 1.0 {
            s.factor[8] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[8] = gain as f64;
            gain = 0.0;
        }
        if gain as f64 > 1.0 {
            s.factor[9] = 1.0;
            gain = (gain as f64 - 1.0) as f32;
        } else {
            s.factor[9] = gain as f64;
            gain = 0.0;
        }
        if (overallscale as f64) < 1.0 {
            overallscale = 1.0;
        }
        s.factor[0] /= overallscale as f64;
        s.factor[1] /= overallscale as f64;
        s.factor[2] /= overallscale as f64;
        s.factor[3] /= overallscale as f64;
        s.factor[4] /= overallscale as f64;
        s.factor[5] /= overallscale as f64;
        s.factor[6] /= overallscale as f64;
        s.factor[7] /= overallscale as f64;
        s.factor[8] /= overallscale as f64;
        s.factor[9] /= overallscale as f64;
        let mut accumulator_sample: f32 = 0.0;
        let mut correction: f32 = 0.0;
        let mut wetness: f32 = s.p[2];
        let mut dryness: f32 = (1.0 - wetness as f64) as f32;
        let mut count: i32 = 0;
        {
            count = 1;
            while count < 165 {
                s.t[count as usize] = (s.p_v[count as usize] as f32 * scale_direct) as i32;
                count += 1;
            }
        }
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
            if s.d_count < 0 || s.d_count > 22050 {
                s.d_count = 22050;
            }
            s.d[s.d_count as usize] = input_sample_l + input_sample_r;
            s.d[(s.d_count + 22050) as usize] = s.d[s.d_count as usize];
            s.d_count -= 1;
            s.duty_cycle += 1;
            if s.duty_cycle as f32 > scale_direct {
                s.duty_cycle = 1;
                s.t[s.pitch_counter as usize] += s.increment;
                s.pitch_counter += 1;
                if s.pitch_counter > range_direct {
                    if s.increment == 1 {
                        s.pitch_counter = 1;
                        if s.t[1] as f32 > 11.0 * scale_direct + 1000.0 {
                            s.increment = -1;
                        }
                    } else {
                        s.pitch_counter = 1;
                        if (s.t[1] as f32) < 11.0 * scale_direct {
                            s.increment = 1;
                            {
                                count = 1;
                                while count < 165 {
                                    s.t[count as usize] = (s.p_v[count as usize] as f32 * scale_direct) as i32;
                                    count += 1;
                                }
                            }
                        }
                    }
                }
            }
            buffer_l = 0;
            buffer_r = 0;
            'sw1: {
                let sw: i32 = range_direct;
                let mut ft = false;
                if ft || sw == 164 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[164]) as usize] * s.out_l[164]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[164]) as usize] * s.out_r[164]) as i32;
                }
                if ft || sw == 163 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[163]) as usize] * s.out_l[163]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[163]) as usize] * s.out_r[163]) as i32;
                }
                if ft || sw == 162 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[162]) as usize] * s.out_l[162]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[162]) as usize] * s.out_r[162]) as i32;
                }
                if ft || sw == 161 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[161]) as usize] * s.out_l[161]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[161]) as usize] * s.out_r[161]) as i32;
                }
                if ft || sw == 160 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[160]) as usize] * s.out_l[160]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[160]) as usize] * s.out_r[160]) as i32;
                }
                if ft || sw == 159 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[159]) as usize] * s.out_l[159]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[159]) as usize] * s.out_r[159]) as i32;
                }
                if ft || sw == 158 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[158]) as usize] * s.out_l[158]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[158]) as usize] * s.out_r[158]) as i32;
                }
                if ft || sw == 157 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[157]) as usize] * s.out_l[157]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[157]) as usize] * s.out_r[157]) as i32;
                }
                if ft || sw == 156 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[156]) as usize] * s.out_l[156]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[156]) as usize] * s.out_r[156]) as i32;
                }
                if ft || sw == 155 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[155]) as usize] * s.out_l[155]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[155]) as usize] * s.out_r[155]) as i32;
                }
                if ft || sw == 154 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[154]) as usize] * s.out_l[154]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[154]) as usize] * s.out_r[154]) as i32;
                }
                if ft || sw == 153 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[153]) as usize] * s.out_l[153]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[153]) as usize] * s.out_r[153]) as i32;
                }
                if ft || sw == 152 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[152]) as usize] * s.out_l[152]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[152]) as usize] * s.out_r[152]) as i32;
                }
                if ft || sw == 151 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[151]) as usize] * s.out_l[151]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[151]) as usize] * s.out_r[151]) as i32;
                }
                if ft || sw == 150 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[150]) as usize] * s.out_l[150]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[150]) as usize] * s.out_r[150]) as i32;
                }
                if ft || sw == 149 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[149]) as usize] * s.out_l[149]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[149]) as usize] * s.out_r[149]) as i32;
                }
                if ft || sw == 148 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[148]) as usize] * s.out_l[148]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[148]) as usize] * s.out_r[148]) as i32;
                }
                if ft || sw == 147 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[147]) as usize] * s.out_l[147]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[147]) as usize] * s.out_r[147]) as i32;
                }
                if ft || sw == 146 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[146]) as usize] * s.out_l[146]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[146]) as usize] * s.out_r[146]) as i32;
                }
                if ft || sw == 145 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[145]) as usize] * s.out_l[145]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[145]) as usize] * s.out_r[145]) as i32;
                }
                if ft || sw == 144 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[144]) as usize] * s.out_l[144]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[144]) as usize] * s.out_r[144]) as i32;
                }
                if ft || sw == 143 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[143]) as usize] * s.out_l[143]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[143]) as usize] * s.out_r[143]) as i32;
                }
                if ft || sw == 142 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[142]) as usize] * s.out_l[142]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[142]) as usize] * s.out_r[142]) as i32;
                }
                if ft || sw == 141 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[141]) as usize] * s.out_l[141]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[141]) as usize] * s.out_r[141]) as i32;
                }
                if ft || sw == 140 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[140]) as usize] * s.out_l[140]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[140]) as usize] * s.out_r[140]) as i32;
                }
                if ft || sw == 139 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[139]) as usize] * s.out_l[139]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[139]) as usize] * s.out_r[139]) as i32;
                }
                if ft || sw == 138 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[138]) as usize] * s.out_l[138]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[138]) as usize] * s.out_r[138]) as i32;
                }
                if ft || sw == 137 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[137]) as usize] * s.out_l[137]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[137]) as usize] * s.out_r[137]) as i32;
                }
                if ft || sw == 136 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[136]) as usize] * s.out_l[136]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[136]) as usize] * s.out_r[136]) as i32;
                }
                if ft || sw == 135 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[135]) as usize] * s.out_l[135]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[135]) as usize] * s.out_r[135]) as i32;
                }
                if ft || sw == 134 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[134]) as usize] * s.out_l[134]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[134]) as usize] * s.out_r[134]) as i32;
                }
                if ft || sw == 133 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[133]) as usize] * s.out_l[133]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[133]) as usize] * s.out_r[133]) as i32;
                }
                if ft || sw == 132 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[132]) as usize] * s.out_l[132]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[132]) as usize] * s.out_r[132]) as i32;
                }
                if ft || sw == 131 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[131]) as usize] * s.out_l[131]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[131]) as usize] * s.out_r[131]) as i32;
                }
                if ft || sw == 130 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[130]) as usize] * s.out_l[130]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[130]) as usize] * s.out_r[130]) as i32;
                }
                if ft || sw == 129 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[129]) as usize] * s.out_l[129]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[129]) as usize] * s.out_r[129]) as i32;
                }
                if ft || sw == 128 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[128]) as usize] * s.out_l[128]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[128]) as usize] * s.out_r[128]) as i32;
                }
                if ft || sw == 127 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[127]) as usize] * s.out_l[127]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[127]) as usize] * s.out_r[127]) as i32;
                }
                if ft || sw == 126 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[126]) as usize] * s.out_l[126]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[126]) as usize] * s.out_r[126]) as i32;
                }
                if ft || sw == 125 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[125]) as usize] * s.out_l[125]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[125]) as usize] * s.out_r[125]) as i32;
                }
                if ft || sw == 124 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[124]) as usize] * s.out_l[124]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[124]) as usize] * s.out_r[124]) as i32;
                }
                if ft || sw == 123 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[123]) as usize] * s.out_l[123]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[123]) as usize] * s.out_r[123]) as i32;
                }
                if ft || sw == 122 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[122]) as usize] * s.out_l[122]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[122]) as usize] * s.out_r[122]) as i32;
                }
                if ft || sw == 121 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[121]) as usize] * s.out_l[121]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[121]) as usize] * s.out_r[121]) as i32;
                }
                if ft || sw == 120 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[120]) as usize] * s.out_l[120]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[120]) as usize] * s.out_r[120]) as i32;
                }
                if ft || sw == 119 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[119]) as usize] * s.out_l[119]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[119]) as usize] * s.out_r[119]) as i32;
                }
                if ft || sw == 118 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[118]) as usize] * s.out_l[118]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[118]) as usize] * s.out_r[118]) as i32;
                }
                if ft || sw == 117 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[117]) as usize] * s.out_l[117]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[117]) as usize] * s.out_r[117]) as i32;
                }
                if ft || sw == 116 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[116]) as usize] * s.out_l[116]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[116]) as usize] * s.out_r[116]) as i32;
                }
                if ft || sw == 115 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[115]) as usize] * s.out_l[115]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[115]) as usize] * s.out_r[115]) as i32;
                }
                if ft || sw == 114 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[114]) as usize] * s.out_l[114]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[114]) as usize] * s.out_r[114]) as i32;
                }
                if ft || sw == 113 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[113]) as usize] * s.out_l[113]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[113]) as usize] * s.out_r[113]) as i32;
                }
                if ft || sw == 112 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[112]) as usize] * s.out_l[112]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[112]) as usize] * s.out_r[112]) as i32;
                }
                if ft || sw == 111 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[111]) as usize] * s.out_l[111]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[111]) as usize] * s.out_r[111]) as i32;
                }
                if ft || sw == 110 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[110]) as usize] * s.out_l[110]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[110]) as usize] * s.out_r[110]) as i32;
                }
                if ft || sw == 109 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[109]) as usize] * s.out_l[109]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[109]) as usize] * s.out_r[109]) as i32;
                }
                if ft || sw == 108 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[108]) as usize] * s.out_l[108]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[108]) as usize] * s.out_r[108]) as i32;
                }
                if ft || sw == 107 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[107]) as usize] * s.out_l[107]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[107]) as usize] * s.out_r[107]) as i32;
                }
                if ft || sw == 106 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[106]) as usize] * s.out_l[106]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[106]) as usize] * s.out_r[106]) as i32;
                }
                if ft || sw == 105 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[105]) as usize] * s.out_l[105]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[105]) as usize] * s.out_r[105]) as i32;
                }
                if ft || sw == 104 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[104]) as usize] * s.out_l[104]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[104]) as usize] * s.out_r[104]) as i32;
                }
                if ft || sw == 103 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[103]) as usize] * s.out_l[103]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[103]) as usize] * s.out_r[103]) as i32;
                }
                if ft || sw == 102 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[102]) as usize] * s.out_l[102]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[102]) as usize] * s.out_r[102]) as i32;
                }
                if ft || sw == 101 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[101]) as usize] * s.out_l[101]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[101]) as usize] * s.out_r[101]) as i32;
                }
                if ft || sw == 100 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[100]) as usize] * s.out_l[100]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[100]) as usize] * s.out_r[100]) as i32;
                }
                if ft || sw == 99 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[99]) as usize] * s.out_l[99]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[99]) as usize] * s.out_r[99]) as i32;
                }
                if ft || sw == 98 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[98]) as usize] * s.out_l[98]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[98]) as usize] * s.out_r[98]) as i32;
                }
                if ft || sw == 97 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[97]) as usize] * s.out_l[97]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[97]) as usize] * s.out_r[97]) as i32;
                }
                if ft || sw == 96 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[96]) as usize] * s.out_l[96]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[96]) as usize] * s.out_r[96]) as i32;
                }
                if ft || sw == 95 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[95]) as usize] * s.out_l[95]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[95]) as usize] * s.out_r[95]) as i32;
                }
                if ft || sw == 94 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[94]) as usize] * s.out_l[94]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[94]) as usize] * s.out_r[94]) as i32;
                }
                if ft || sw == 93 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[93]) as usize] * s.out_l[93]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[93]) as usize] * s.out_r[93]) as i32;
                }
                if ft || sw == 92 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[92]) as usize] * s.out_l[92]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[92]) as usize] * s.out_r[92]) as i32;
                }
                if ft || sw == 91 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[91]) as usize] * s.out_l[91]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[91]) as usize] * s.out_r[91]) as i32;
                }
                if ft || sw == 90 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[90]) as usize] * s.out_l[90]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[90]) as usize] * s.out_r[90]) as i32;
                }
                if ft || sw == 89 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[89]) as usize] * s.out_l[89]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[89]) as usize] * s.out_r[89]) as i32;
                }
                if ft || sw == 88 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[88]) as usize] * s.out_l[88]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[88]) as usize] * s.out_r[88]) as i32;
                }
                if ft || sw == 87 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[87]) as usize] * s.out_l[87]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[87]) as usize] * s.out_r[87]) as i32;
                }
                if ft || sw == 86 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[86]) as usize] * s.out_l[86]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[86]) as usize] * s.out_r[86]) as i32;
                }
                if ft || sw == 85 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[85]) as usize] * s.out_l[85]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[85]) as usize] * s.out_r[85]) as i32;
                }
                if ft || sw == 84 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[84]) as usize] * s.out_l[84]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[84]) as usize] * s.out_r[84]) as i32;
                }
                if ft || sw == 83 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[83]) as usize] * s.out_l[83]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[83]) as usize] * s.out_r[83]) as i32;
                }
                if ft || sw == 82 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[82]) as usize] * s.out_l[82]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[82]) as usize] * s.out_r[82]) as i32;
                }
                if ft || sw == 81 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[81]) as usize] * s.out_l[81]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[81]) as usize] * s.out_r[81]) as i32;
                }
                if ft || sw == 80 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[80]) as usize] * s.out_l[80]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[80]) as usize] * s.out_r[80]) as i32;
                }
                if ft || sw == 79 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[79]) as usize] * s.out_l[79]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[79]) as usize] * s.out_r[79]) as i32;
                }
                if ft || sw == 78 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[78]) as usize] * s.out_l[78]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[78]) as usize] * s.out_r[78]) as i32;
                }
                if ft || sw == 77 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[77]) as usize] * s.out_l[77]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[77]) as usize] * s.out_r[77]) as i32;
                }
                if ft || sw == 76 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[76]) as usize] * s.out_l[76]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[76]) as usize] * s.out_r[76]) as i32;
                }
                if ft || sw == 75 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[75]) as usize] * s.out_l[75]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[75]) as usize] * s.out_r[75]) as i32;
                }
                if ft || sw == 74 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[74]) as usize] * s.out_l[74]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[74]) as usize] * s.out_r[74]) as i32;
                }
                if ft || sw == 73 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[73]) as usize] * s.out_l[73]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[73]) as usize] * s.out_r[73]) as i32;
                }
                if ft || sw == 72 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[72]) as usize] * s.out_l[72]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[72]) as usize] * s.out_r[72]) as i32;
                }
                if ft || sw == 71 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[71]) as usize] * s.out_l[71]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[71]) as usize] * s.out_r[71]) as i32;
                }
                if ft || sw == 70 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[70]) as usize] * s.out_l[70]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[70]) as usize] * s.out_r[70]) as i32;
                }
                if ft || sw == 69 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[69]) as usize] * s.out_l[69]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[69]) as usize] * s.out_r[69]) as i32;
                }
                if ft || sw == 68 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[68]) as usize] * s.out_l[68]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[68]) as usize] * s.out_r[68]) as i32;
                }
                if ft || sw == 67 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[67]) as usize] * s.out_l[67]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[67]) as usize] * s.out_r[67]) as i32;
                }
                if ft || sw == 66 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[66]) as usize] * s.out_l[66]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[66]) as usize] * s.out_r[66]) as i32;
                }
                if ft || sw == 65 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[65]) as usize] * s.out_l[65]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[65]) as usize] * s.out_r[65]) as i32;
                }
                if ft || sw == 64 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[64]) as usize] * s.out_l[64]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[64]) as usize] * s.out_r[64]) as i32;
                }
                if ft || sw == 63 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[63]) as usize] * s.out_l[63]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[63]) as usize] * s.out_r[63]) as i32;
                }
                if ft || sw == 62 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[62]) as usize] * s.out_l[62]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[62]) as usize] * s.out_r[62]) as i32;
                }
                if ft || sw == 61 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[61]) as usize] * s.out_l[61]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[61]) as usize] * s.out_r[61]) as i32;
                }
                if ft || sw == 60 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[60]) as usize] * s.out_l[60]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[60]) as usize] * s.out_r[60]) as i32;
                }
                if ft || sw == 59 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[59]) as usize] * s.out_l[59]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[59]) as usize] * s.out_r[59]) as i32;
                }
                if ft || sw == 58 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[58]) as usize] * s.out_l[58]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[58]) as usize] * s.out_r[58]) as i32;
                }
                if ft || sw == 57 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[57]) as usize] * s.out_l[57]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[57]) as usize] * s.out_r[57]) as i32;
                }
                if ft || sw == 56 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[56]) as usize] * s.out_l[56]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[56]) as usize] * s.out_r[56]) as i32;
                }
                if ft || sw == 55 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[55]) as usize] * s.out_l[55]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[55]) as usize] * s.out_r[55]) as i32;
                }
                if ft || sw == 54 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[54]) as usize] * s.out_l[54]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[54]) as usize] * s.out_r[54]) as i32;
                }
                if ft || sw == 53 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[53]) as usize] * s.out_l[53]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[53]) as usize] * s.out_r[53]) as i32;
                }
                if ft || sw == 52 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[52]) as usize] * s.out_l[52]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[52]) as usize] * s.out_r[52]) as i32;
                }
                if ft || sw == 51 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[51]) as usize] * s.out_l[51]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[51]) as usize] * s.out_r[51]) as i32;
                }
                if ft || sw == 50 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[50]) as usize] * s.out_l[50]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[50]) as usize] * s.out_r[50]) as i32;
                }
                if ft || sw == 49 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[49]) as usize] * s.out_l[49]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[49]) as usize] * s.out_r[49]) as i32;
                }
                if ft || sw == 48 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[48]) as usize] * s.out_l[48]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[48]) as usize] * s.out_r[48]) as i32;
                }
                if ft || sw == 47 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[47]) as usize] * s.out_l[47]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[47]) as usize] * s.out_r[47]) as i32;
                }
                if ft || sw == 46 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[46]) as usize] * s.out_l[46]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[46]) as usize] * s.out_r[46]) as i32;
                }
                if ft || sw == 45 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[45]) as usize] * s.out_l[45]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[45]) as usize] * s.out_r[45]) as i32;
                }
                if ft || sw == 44 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[44]) as usize] * s.out_l[44]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[44]) as usize] * s.out_r[44]) as i32;
                }
                if ft || sw == 43 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[43]) as usize] * s.out_l[43]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[43]) as usize] * s.out_r[43]) as i32;
                }
                if ft || sw == 42 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[42]) as usize] * s.out_l[42]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[42]) as usize] * s.out_r[42]) as i32;
                }
                if ft || sw == 41 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[41]) as usize] * s.out_l[41]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[41]) as usize] * s.out_r[41]) as i32;
                }
                if ft || sw == 40 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[40]) as usize] * s.out_l[40]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[40]) as usize] * s.out_r[40]) as i32;
                }
                if ft || sw == 39 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[39]) as usize] * s.out_l[39]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[39]) as usize] * s.out_r[39]) as i32;
                }
                if ft || sw == 38 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[38]) as usize] * s.out_l[38]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[38]) as usize] * s.out_r[38]) as i32;
                }
                if ft || sw == 37 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[37]) as usize] * s.out_l[37]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[37]) as usize] * s.out_r[37]) as i32;
                }
                if ft || sw == 36 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[36]) as usize] * s.out_l[36]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[36]) as usize] * s.out_r[36]) as i32;
                }
                if ft || sw == 35 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[35]) as usize] * s.out_l[35]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[35]) as usize] * s.out_r[35]) as i32;
                }
                if ft || sw == 34 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[34]) as usize] * s.out_l[34]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[34]) as usize] * s.out_r[34]) as i32;
                }
                if ft || sw == 33 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[33]) as usize] * s.out_l[33]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[33]) as usize] * s.out_r[33]) as i32;
                }
                if ft || sw == 32 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[32]) as usize] * s.out_l[32]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[32]) as usize] * s.out_r[32]) as i32;
                }
                if ft || sw == 31 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[31]) as usize] * s.out_l[31]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[31]) as usize] * s.out_r[31]) as i32;
                }
                if ft || sw == 30 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[30]) as usize] * s.out_l[30]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[30]) as usize] * s.out_r[30]) as i32;
                }
                if ft || sw == 29 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[29]) as usize] * s.out_l[29]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[29]) as usize] * s.out_r[29]) as i32;
                }
                if ft || sw == 28 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[28]) as usize] * s.out_l[28]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[28]) as usize] * s.out_r[28]) as i32;
                }
                if ft || sw == 27 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[27]) as usize] * s.out_l[27]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[27]) as usize] * s.out_r[27]) as i32;
                }
                if ft || sw == 26 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[26]) as usize] * s.out_l[26]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[26]) as usize] * s.out_r[26]) as i32;
                }
                if ft || sw == 25 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[25]) as usize] * s.out_l[25]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[25]) as usize] * s.out_r[25]) as i32;
                }
                if ft || sw == 24 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[24]) as usize] * s.out_l[24]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[24]) as usize] * s.out_r[24]) as i32;
                }
                if ft || sw == 23 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[23]) as usize] * s.out_l[23]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[23]) as usize] * s.out_r[23]) as i32;
                }
                if ft || sw == 22 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[22]) as usize] * s.out_l[22]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[22]) as usize] * s.out_r[22]) as i32;
                }
                if ft || sw == 21 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[21]) as usize] * s.out_l[21]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[21]) as usize] * s.out_r[21]) as i32;
                }
                if ft || sw == 20 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[20]) as usize] * s.out_l[20]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[20]) as usize] * s.out_r[20]) as i32;
                }
                if ft || sw == 19 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[19]) as usize] * s.out_l[19]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[19]) as usize] * s.out_r[19]) as i32;
                }
                if ft || sw == 18 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[18]) as usize] * s.out_l[18]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[18]) as usize] * s.out_r[18]) as i32;
                }
                if ft || sw == 17 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[17]) as usize] * s.out_l[17]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[17]) as usize] * s.out_r[17]) as i32;
                }
                if ft || sw == 16 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[16]) as usize] * s.out_l[16]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[16]) as usize] * s.out_r[16]) as i32;
                }
                if ft || sw == 15 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[15]) as usize] * s.out_l[15]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[15]) as usize] * s.out_r[15]) as i32;
                }
                if ft || sw == 14 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[14]) as usize] * s.out_l[14]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[14]) as usize] * s.out_r[14]) as i32;
                }
                if ft || sw == 13 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[13]) as usize] * s.out_l[13]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[13]) as usize] * s.out_r[13]) as i32;
                }
                if ft || sw == 12 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[12]) as usize] * s.out_l[12]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[12]) as usize] * s.out_r[12]) as i32;
                }
                if ft || sw == 11 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[11]) as usize] * s.out_l[11]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[11]) as usize] * s.out_r[11]) as i32;
                }
                if ft || sw == 10 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[10]) as usize] * s.out_l[10]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[10]) as usize] * s.out_r[10]) as i32;
                }
                if ft || sw == 9 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[9]) as usize] * s.out_l[9]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[9]) as usize] * s.out_r[9]) as i32;
                }
                if ft || sw == 8 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[8]) as usize] * s.out_l[8]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[8]) as usize] * s.out_r[8]) as i32;
                }
                if ft || sw == 7 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[7]) as usize] * s.out_l[7]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[7]) as usize] * s.out_r[7]) as i32;
                }
                if ft || sw == 6 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[6]) as usize] * s.out_l[6]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[6]) as usize] * s.out_r[6]) as i32;
                }
                if ft || sw == 5 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[5]) as usize] * s.out_l[5]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[5]) as usize] * s.out_r[5]) as i32;
                }
                if ft || sw == 4 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[4]) as usize] * s.out_l[4]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[4]) as usize] * s.out_r[4]) as i32;
                }
                if ft || sw == 3 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[3]) as usize] * s.out_l[3]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[3]) as usize] * s.out_r[3]) as i32;
                }
                if ft || sw == 2 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[2]) as usize] * s.out_l[2]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[2]) as usize] * s.out_r[2]) as i32;
                }
                if ft || sw == 1 {
                    ft = true;
                    buffer_l += (s.d[(s.d_count + s.t[1]) as usize] * s.out_l[1]) as i32;
                    buffer_r += (s.d[(s.d_count + s.t[1]) as usize] * s.out_r[1]) as i32;
                }
            }
            input_sample_l = buffer_l as f64;
            input_sample_r = buffer_r as f64;
            s.wear_r[9] = s.wear_r[8];
            s.wear_r[8] = s.wear_r[7];
            s.wear_r[7] = s.wear_r[6];
            s.wear_r[6] = s.wear_r[5];
            s.wear_r[5] = s.wear_r[4];
            s.wear_r[4] = s.wear_r[3];
            s.wear_r[3] = s.wear_r[2];
            s.wear_r[2] = s.wear_r[1];
            s.wear_r[1] = s.wear_r[0];
            accumulator_sample = (input_sample_r - s.wear_rprev) as f32;
            s.wear_r[0] = accumulator_sample as f64;
            accumulator_sample = (accumulator_sample as f64 * s.factor[0]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_r[1] * s.factor[1]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_r[2] * s.factor[2]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_r[3] * s.factor[3]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_r[4] * s.factor[4]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_r[5] * s.factor[5]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_r[6] * s.factor[6]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_r[7] * s.factor[7]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_r[8] * s.factor[8]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_r[9] * s.factor[9]) as f32;
            correction = (input_sample_r - s.wear_rprev + accumulator_sample as f64) as f32;
            s.wear_rprev = input_sample_r;
            input_sample_r += correction as f64;
            s.wear_l[9] = s.wear_l[8];
            s.wear_l[8] = s.wear_l[7];
            s.wear_l[7] = s.wear_l[6];
            s.wear_l[6] = s.wear_l[5];
            s.wear_l[5] = s.wear_l[4];
            s.wear_l[4] = s.wear_l[3];
            s.wear_l[3] = s.wear_l[2];
            s.wear_l[2] = s.wear_l[1];
            s.wear_l[1] = s.wear_l[0];
            accumulator_sample = (input_sample_l - s.wear_lprev) as f32;
            s.wear_l[0] = accumulator_sample as f64;
            accumulator_sample = (accumulator_sample as f64 * s.factor[0]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_l[1] * s.factor[1]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_l[2] * s.factor[2]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_l[3] * s.factor[3]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_l[4] * s.factor[4]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_l[5] * s.factor[5]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_l[6] * s.factor[6]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_l[7] * s.factor[7]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_l[8] * s.factor[8]) as f32;
            accumulator_sample = (accumulator_sample as f64 + s.wear_l[9] * s.factor[9]) as f32;
            correction = (input_sample_l - s.wear_lprev + accumulator_sample as f64) as f32;
            s.wear_lprev = input_sample_l;
            input_sample_l += correction as f64;
            input_sample_l /= output_pad as f64;
            input_sample_r /= output_pad as f64;
            dry_sample_l *= dryness as f64;
            dry_sample_r *= dryness as f64;
            input_sample_l *= wetness as f64;
            input_sample_r *= wetness as f64;
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
            l[smp] = input_sample_l as f32;
            r[smp] = input_sample_r as f32;
        }
    }
}
