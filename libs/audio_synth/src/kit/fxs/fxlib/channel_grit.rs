// Ported from Apicolypse (ApicolypseProc.cpp, Apicolypse.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Apicolypse (sub-effect id 26): the upstream float processing path, ported as written
//! (double maths where upstream uses double, its own denormal guard and dither state).

use super::{CRand, Sub, frexpf_exp, pf};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[
    pf("Hardns", 0.7),
    pf("Persnlty", 0.3333333),
    pf("Drive", 0.3333333),
    pf("Output", 1.0),
];

#[allow(dead_code)]
pub struct ChannelGrit {
    p: [f32; 4],
    sr: f32,
    b_r: [f64; 35],
    last_sample_r: f64,
    b_l: [f64; 35],
    last_sample_l: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl ChannelGrit {
    #[allow(unused_variables, unused_mut, unused_assignments)]
    pub fn new(sr: f64, rng: &mut CRand) -> Box<ChannelGrit> {
        let mut s = Box::new(ChannelGrit {
            p: [0.7, 0.3333333, 0.3333333, 1.0],
            sr: sr as f32,
            b_r: [0.0; 35],
            last_sample_r: 0.0,
            b_l: [0.0; 35],
            last_sample_l: 0.0,
            fpd_l: 0,
            fpd_r: 0,
        });
        s.fpd_l = rng.fpd();
        s.fpd_r = rng.fpd();
        s
    }
}

impl Sub for ChannelGrit {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    #[allow(unused_variables, unused_mut, unused_assignments, unused_parens, unused_labels)]
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut *self;
        let mut threshold: f64 = s.p[0] as f64;
        let mut hardness: f64 = 0.0;
        let mut breakup: f64 = (1.0 - threshold / 2.0) * 3.14159265358979;
        let mut bridgerectifier: f64 = 0.0;
        let mut sqdrive: f64 = s.p[1] as f64 * 3.0;
        if sqdrive > 1.0 {
            sqdrive *= sqdrive;
        }
        sqdrive = sqdrive.sqrt();
        let mut indrive: f64 = s.p[2] as f64 * 3.0;
        if indrive > 1.0 {
            indrive *= indrive;
        }
        indrive *= 1.0 - 0.008 * sqdrive;
        let mut randy: f64 = 0.0;
        let mut outlevel: f64 = s.p[3] as f64;
        if threshold < 1.0 {
            hardness = 1.0 / (1.0 - threshold);
        } else {
            hardness = 999999999999999999999.0;
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
            input_sample_l *= indrive;
            input_sample_r *= indrive;
            if sqdrive > 0.0 {
                s.b_l[33] = s.b_l[32];
                s.b_l[32] = s.b_l[31];
                s.b_l[31] = s.b_l[30];
                s.b_l[30] = s.b_l[29];
                s.b_l[29] = s.b_l[28];
                s.b_l[28] = s.b_l[27];
                s.b_l[27] = s.b_l[26];
                s.b_l[26] = s.b_l[25];
                s.b_l[25] = s.b_l[24];
                s.b_l[24] = s.b_l[23];
                s.b_l[23] = s.b_l[22];
                s.b_l[22] = s.b_l[21];
                s.b_l[21] = s.b_l[20];
                s.b_l[20] = s.b_l[19];
                s.b_l[19] = s.b_l[18];
                s.b_l[18] = s.b_l[17];
                s.b_l[17] = s.b_l[16];
                s.b_l[16] = s.b_l[15];
                s.b_l[15] = s.b_l[14];
                s.b_l[14] = s.b_l[13];
                s.b_l[13] = s.b_l[12];
                s.b_l[12] = s.b_l[11];
                s.b_l[11] = s.b_l[10];
                s.b_l[10] = s.b_l[9];
                s.b_l[9] = s.b_l[8];
                s.b_l[8] = s.b_l[7];
                s.b_l[7] = s.b_l[6];
                s.b_l[6] = s.b_l[5];
                s.b_l[5] = s.b_l[4];
                s.b_l[4] = s.b_l[3];
                s.b_l[3] = s.b_l[2];
                s.b_l[2] = s.b_l[1];
                s.b_l[1] = s.b_l[0];
                s.b_l[0] = input_sample_l * sqdrive;
                input_sample_l += s.b_l[1] * (0.09299870608542582 - 0.00009582362368873 * s.b_l[1].abs());
                input_sample_l -= s.b_l[2] * (0.11947847710741009 - 0.00004500891602770 * s.b_l[2].abs());
                input_sample_l += s.b_l[3] * (0.09071606264761795 + 0.00005639498984741 * s.b_l[3].abs());
                input_sample_l -= s.b_l[4] * (0.08561982770836980 - 0.00004964855606916 * s.b_l[4].abs());
                input_sample_l += s.b_l[5] * (0.06440549220820363 + 0.00002428052139507 * s.b_l[5].abs());
                input_sample_l -= s.b_l[6] * (0.05987991812840746 + 0.00000101867082290 * s.b_l[6].abs());
                input_sample_l += s.b_l[7] * (0.03980233135839382 + 0.00003312430049041 * s.b_l[7].abs());
                input_sample_l -= s.b_l[8] * (0.03648402630896925 - 0.00002116186381142 * s.b_l[8].abs());
                input_sample_l += s.b_l[9] * (0.01826860869525248 + 0.00003115110025396 * s.b_l[9].abs());
                input_sample_l -= s.b_l[10] * (0.01723968622495364 - 0.00002450634121718 * s.b_l[10].abs());
                input_sample_l += s.b_l[11] * (0.00187588812316724 + 0.00002838206198968 * s.b_l[11].abs());
                input_sample_l -= s.b_l[12] * (0.00381796423957237 - 0.00003155815499462 * s.b_l[12].abs());
                input_sample_l -= s.b_l[13] * (0.00852092214496733 - 0.00001702651162392 * s.b_l[13].abs());
                input_sample_l += s.b_l[14] * (0.00315560292270588 + 0.00002547861676047 * s.b_l[14].abs());
                input_sample_l -= s.b_l[15] * (0.01258630914496868 - 0.00004555319243213 * s.b_l[15].abs());
                input_sample_l += s.b_l[16] * (0.00536435648963575 + 0.00001812393657101 * s.b_l[16].abs());
                input_sample_l -= s.b_l[17] * (0.01272975658159178 - 0.00004103775306121 * s.b_l[17].abs());
                input_sample_l += s.b_l[18] * (0.00403818975172755 + 0.00003764615492871 * s.b_l[18].abs());
                input_sample_l -= s.b_l[19] * (0.01042617366897483 - 0.00003605210426041 * s.b_l[19].abs());
                input_sample_l += s.b_l[20] * (0.00126599583390057 + 0.00004305458668852 * s.b_l[20].abs());
                input_sample_l -= s.b_l[21] * (0.00747876207688339 - 0.00003731207018977 * s.b_l[21].abs());
                input_sample_l -= s.b_l[22] * (0.00149873689175324 - 0.00005086601800791 * s.b_l[22].abs());
                input_sample_l -= s.b_l[23] * (0.00503221309488033 - 0.00003636086782783 * s.b_l[23].abs());
                input_sample_l -= s.b_l[24] * (0.00342998224655821 - 0.00004103091180506 * s.b_l[24].abs());
                input_sample_l -= s.b_l[25] * (0.00355585977903117 - 0.00003698982145400 * s.b_l[25].abs());
                input_sample_l -= s.b_l[26] * (0.00437201792934817 - 0.00002720235666939 * s.b_l[26].abs());
                input_sample_l -= s.b_l[27] * (0.00299217874451556 - 0.00004446954727956 * s.b_l[27].abs());
                input_sample_l -= s.b_l[28] * (0.00457924652487249 - 0.00003859065778860 * s.b_l[28].abs());
                input_sample_l -= s.b_l[29] * (0.00298182934892027 - 0.00002064710931733 * s.b_l[29].abs());
                input_sample_l -= s.b_l[30] * (0.00438838441540584 - 0.00005223008424866 * s.b_l[30].abs());
                input_sample_l -= s.b_l[31] * (0.00323984218794705 - 0.00003397987535887 * s.b_l[31].abs());
                input_sample_l -= s.b_l[32] * (0.00407693981307314 - 0.00003935772436894 * s.b_l[32].abs());
                input_sample_l -= s.b_l[33] * (0.00350435348467321 - 0.00005525463935338 * s.b_l[33].abs());
                s.b_r[33] = s.b_r[32];
                s.b_r[32] = s.b_r[31];
                s.b_r[31] = s.b_r[30];
                s.b_r[30] = s.b_r[29];
                s.b_r[29] = s.b_r[28];
                s.b_r[28] = s.b_r[27];
                s.b_r[27] = s.b_r[26];
                s.b_r[26] = s.b_r[25];
                s.b_r[25] = s.b_r[24];
                s.b_r[24] = s.b_r[23];
                s.b_r[23] = s.b_r[22];
                s.b_r[22] = s.b_r[21];
                s.b_r[21] = s.b_r[20];
                s.b_r[20] = s.b_r[19];
                s.b_r[19] = s.b_r[18];
                s.b_r[18] = s.b_r[17];
                s.b_r[17] = s.b_r[16];
                s.b_r[16] = s.b_r[15];
                s.b_r[15] = s.b_r[14];
                s.b_r[14] = s.b_r[13];
                s.b_r[13] = s.b_r[12];
                s.b_r[12] = s.b_r[11];
                s.b_r[11] = s.b_r[10];
                s.b_r[10] = s.b_r[9];
                s.b_r[9] = s.b_r[8];
                s.b_r[8] = s.b_r[7];
                s.b_r[7] = s.b_r[6];
                s.b_r[6] = s.b_r[5];
                s.b_r[5] = s.b_r[4];
                s.b_r[4] = s.b_r[3];
                s.b_r[3] = s.b_r[2];
                s.b_r[2] = s.b_r[1];
                s.b_r[1] = s.b_r[0];
                s.b_r[0] = input_sample_r * sqdrive;
                input_sample_r += s.b_r[1] * (0.09299870608542582 - 0.00009582362368873 * s.b_r[1].abs());
                input_sample_r -= s.b_r[2] * (0.11947847710741009 - 0.00004500891602770 * s.b_r[2].abs());
                input_sample_r += s.b_r[3] * (0.09071606264761795 + 0.00005639498984741 * s.b_r[3].abs());
                input_sample_r -= s.b_r[4] * (0.08561982770836980 - 0.00004964855606916 * s.b_r[4].abs());
                input_sample_r += s.b_r[5] * (0.06440549220820363 + 0.00002428052139507 * s.b_r[5].abs());
                input_sample_r -= s.b_r[6] * (0.05987991812840746 + 0.00000101867082290 * s.b_r[6].abs());
                input_sample_r += s.b_r[7] * (0.03980233135839382 + 0.00003312430049041 * s.b_r[7].abs());
                input_sample_r -= s.b_r[8] * (0.03648402630896925 - 0.00002116186381142 * s.b_r[8].abs());
                input_sample_r += s.b_r[9] * (0.01826860869525248 + 0.00003115110025396 * s.b_r[9].abs());
                input_sample_r -= s.b_r[10] * (0.01723968622495364 - 0.00002450634121718 * s.b_r[10].abs());
                input_sample_r += s.b_r[11] * (0.00187588812316724 + 0.00002838206198968 * s.b_r[11].abs());
                input_sample_r -= s.b_r[12] * (0.00381796423957237 - 0.00003155815499462 * s.b_r[12].abs());
                input_sample_r -= s.b_r[13] * (0.00852092214496733 - 0.00001702651162392 * s.b_r[13].abs());
                input_sample_r += s.b_r[14] * (0.00315560292270588 + 0.00002547861676047 * s.b_r[14].abs());
                input_sample_r -= s.b_r[15] * (0.01258630914496868 - 0.00004555319243213 * s.b_r[15].abs());
                input_sample_r += s.b_r[16] * (0.00536435648963575 + 0.00001812393657101 * s.b_r[16].abs());
                input_sample_r -= s.b_r[17] * (0.01272975658159178 - 0.00004103775306121 * s.b_r[17].abs());
                input_sample_r += s.b_r[18] * (0.00403818975172755 + 0.00003764615492871 * s.b_r[18].abs());
                input_sample_r -= s.b_r[19] * (0.01042617366897483 - 0.00003605210426041 * s.b_r[19].abs());
                input_sample_r += s.b_r[20] * (0.00126599583390057 + 0.00004305458668852 * s.b_r[20].abs());
                input_sample_r -= s.b_r[21] * (0.00747876207688339 - 0.00003731207018977 * s.b_r[21].abs());
                input_sample_r -= s.b_r[22] * (0.00149873689175324 - 0.00005086601800791 * s.b_r[22].abs());
                input_sample_r -= s.b_r[23] * (0.00503221309488033 - 0.00003636086782783 * s.b_r[23].abs());
                input_sample_r -= s.b_r[24] * (0.00342998224655821 - 0.00004103091180506 * s.b_r[24].abs());
                input_sample_r -= s.b_r[25] * (0.00355585977903117 - 0.00003698982145400 * s.b_r[25].abs());
                input_sample_r -= s.b_r[26] * (0.00437201792934817 - 0.00002720235666939 * s.b_r[26].abs());
                input_sample_r -= s.b_r[27] * (0.00299217874451556 - 0.00004446954727956 * s.b_r[27].abs());
                input_sample_r -= s.b_r[28] * (0.00457924652487249 - 0.00003859065778860 * s.b_r[28].abs());
                input_sample_r -= s.b_r[29] * (0.00298182934892027 - 0.00002064710931733 * s.b_r[29].abs());
                input_sample_r -= s.b_r[30] * (0.00438838441540584 - 0.00005223008424866 * s.b_r[30].abs());
                input_sample_r -= s.b_r[31] * (0.00323984218794705 - 0.00003397987535887 * s.b_r[31].abs());
                input_sample_r -= s.b_r[32] * (0.00407693981307314 - 0.00003935772436894 * s.b_r[32].abs());
                input_sample_r -= s.b_r[33] * (0.00350435348467321 - 0.00005525463935338 * s.b_r[33].abs());
            }
            if input_sample_l.abs() > threshold {
                bridgerectifier = (input_sample_l.abs() - threshold) * hardness;
                if bridgerectifier > breakup {
                    bridgerectifier = breakup;
                }
                bridgerectifier = bridgerectifier.sin() / hardness;
                if input_sample_l > 0.0 {
                    input_sample_l = bridgerectifier + threshold;
                } else {
                    input_sample_l = -(bridgerectifier + threshold);
                }
            }
            if input_sample_r.abs() > threshold {
                bridgerectifier = (input_sample_r.abs() - threshold) * hardness;
                if bridgerectifier > breakup {
                    bridgerectifier = breakup;
                }
                bridgerectifier = bridgerectifier.sin() / hardness;
                if input_sample_r > 0.0 {
                    input_sample_r = bridgerectifier + threshold;
                } else {
                    input_sample_r = -(bridgerectifier + threshold);
                }
            }
            randy = s.fpd_l as f64 / u32::MAX as f64 * 0.033;
            input_sample_l = (input_sample_l * (1.0 - randy) + s.last_sample_l * randy) * outlevel;
            s.last_sample_l = input_sample_l;
            randy = s.fpd_r as f64 / u32::MAX as f64 * 0.033;
            input_sample_r = (input_sample_r * (1.0 - randy) + s.last_sample_r * randy) * outlevel;
            s.last_sample_r = input_sample_r;
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
