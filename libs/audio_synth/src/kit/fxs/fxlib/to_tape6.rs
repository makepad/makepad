// Ported from ToTape6 (ToTape6Proc.cpp, ToTape6.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Tape: flutter, soften, head bump, a soft fold and an anticipating
//! soft clip.

use super::{denorm, dither32, pf, CRand, Sub};
use crate::kit::core::SlotDef;
use std::f64::consts::PI;

pub const PARAMS: &[SlotDef] = &[
    pf("Input", 0.5),
    pf("Soften", 0.5),
    pf("Head B", 0.5),
    pf("Flutter", 0.5),
    pf("Output", 0.5),
    pf("Dry/Wet", 1.0),
];

#[derive(Clone, Copy, Default)]
struct Side {
    mid_roller_a: f64,
    mid_roller_b: f64,
    head_bump_a: f64,
    head_bump_b: f64,
    bq_a: [f64; 9],
    bq_b: [f64; 9],
    bq_c: [f64; 9],
    bq_d: [f64; 9],
    last: f64,
}

/// The interleaved biquad step upstream writes inline, then the clamp + asin.
#[inline]
fn bq_asin(bq: &mut [f64; 9], x: f64) -> f64 {
    let t = (x * bq[2]) + bq[7];
    bq[7] = (x * bq[3]) - (t * bq[5]) + bq[8];
    bq[8] = (x * bq[4]) - (t * bq[6]);
    t.clamp(-1.0, 1.0).asin()
}

#[inline]
fn set_bq(bq: &mut [f64; 9], freq: f64, reso: f64) {
    bq[0] = freq;
    bq[1] = reso;
    let k = (PI * bq[0]).tan();
    let norm = 1.0 / (1.0 + k / bq[1] + k * k);
    bq[2] = k / bq[1] * norm;
    bq[4] = -bq[2];
    bq[5] = 2.0 * (k * k - 1.0) * norm;
    bq[6] = (1.0 - k / bq[1] + k * k) * norm;
}

pub struct ToTape6 {
    p: [f32; 6],
    sr: f64,
    s: [Side; 2],
    d: [[f64; 502]; 2],
    flip: bool,
    gcount: i32,
    rateof: f64,
    sweep: f64,
    nextmax: f64,
    fpd: [u32; 2],
}

impl ToTape6 {
    pub fn new(sr: f64, rng: &mut CRand) -> Box<ToTape6> {
        let fpd_l = rng.fpd();
        let fpd_r = rng.fpd();
        Box::new(ToTape6 {
            p: [0.5, 0.5, 0.5, 0.5, 0.5, 1.0],
            sr,
            s: [Side::default(); 2],
            d: [[0.0; 502]; 2],
            flip: false,
            gcount: 0,
            rateof: 0.5,
            sweep: PI,
            nextmax: 0.5,
            fpd: [fpd_l, fpd_r],
        })
    }
}

impl Sub for ToTape6 {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let [a, b, c, d, e, f] = self.p.map(|v| v as f64);
        let mut overallscale = 1.0f64;
        overallscale /= 44100.0;
        overallscale *= self.sr;

        let inputgain = 10f64.powf(((a - 0.5) * 24.0) / 20.0);
        let soften_control = b.powf(2.0);
        let roll_amount = (1.0 - (soften_control * 0.45)) / overallscale;
        let head_bump_control = c * 0.25 * inputgain;
        let head_bump_freq = 0.12 / overallscale;
        for s in self.s.iter_mut() {
            set_bq(&mut s.bq_a, 0.007 / overallscale, 0.0009);
            set_bq(&mut s.bq_b, 0.007 / overallscale, 0.0009);
            set_bq(&mut s.bq_c, 0.032 / overallscale, 0.0007);
            set_bq(&mut s.bq_d, 0.032 / overallscale, 0.0007);
        }
        let depth = d.powf(2.0) * overallscale * 70.0;
        let fluttertrim = (0.0024 * d.powf(2.0)) / overallscale;
        let outputgain = 10f64.powf(((e - 0.5) * 24.0) / 20.0);
        let refclip = 0.99;
        let softness = 0.618033988749894848204586;
        let wet = f;

        for (sl, sr) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let mut x = [denorm(*sl as f64, self.fpd[0]), denorm(*sr as f64, self.fpd[1])];
            let dry = x;
            if inputgain < 1.0 {
                x[0] *= inputgain;
                x[1] *= inputgain;
            }

            let flutterrandy = self.fpd[0] as f64 / u32::MAX as f64;
            if self.gcount < 0 || self.gcount > 499 {
                self.gcount = 499;
            }
            let g = self.gcount as usize;
            self.d[0][g] = x[0];
            self.d[1][g] = x[1];
            let mut count = self.gcount;
            if depth != 0.0 {
                let offset = depth + (depth * self.rateof.powf(2.0) * self.sweep.sin());
                count += offset.floor() as i32;
                let fr = offset - offset.floor();
                // upstream reads past its 502-entry line only at very high rates (undefined there); clamp
                let i0 = ((count - if count > 499 { 500 } else { 0 }) as usize).min(501);
                let i1 = ((count + 1 - if count + 1 > 499 { 500 } else { 0 }) as usize).min(501);
                for ch in 0..2 {
                    x[ch] = self.d[ch][i0] * (1.0 - fr);
                    x[ch] += self.d[ch][i1] * fr;
                }
                self.rateof = (self.rateof * (1.0 - fluttertrim)) + (self.nextmax * fluttertrim);
                self.sweep += self.rateof * fluttertrim;
                if self.sweep >= PI * 2.0 {
                    self.sweep -= PI;
                    self.nextmax = 0.24 + (flutterrandy * 0.74);
                }
            }
            self.gcount -= 1;

            let vib_dry = x;
            let mut highs = [0.0f64; 2];
            for ch in 0..2 {
                let s = &mut self.s[ch];
                let xi = x[ch];
                if self.flip {
                    s.mid_roller_a = (s.mid_roller_a * (1.0 - roll_amount)) + (xi * roll_amount);
                    highs[ch] = xi - s.mid_roller_a;
                    s.head_bump_a += xi * 0.05;
                    s.head_bump_a -= s.head_bump_a * s.head_bump_a * s.head_bump_a * head_bump_freq;
                    s.head_bump_a = s.head_bump_a.sin();
                    s.head_bump_a = bq_asin(&mut s.bq_a, s.head_bump_a);
                    x[ch] = bq_asin(&mut s.bq_c, xi.sin());
                } else {
                    s.mid_roller_b = (s.mid_roller_b * (1.0 - roll_amount)) + (xi * roll_amount);
                    highs[ch] = xi - s.mid_roller_b;
                    s.head_bump_b += xi * 0.05;
                    s.head_bump_b -= s.head_bump_b * s.head_bump_b * s.head_bump_b * head_bump_freq;
                    s.head_bump_b = s.head_bump_b.sin();
                    s.head_bump_b = bq_asin(&mut s.bq_b, s.head_bump_b);
                    x[ch] = bq_asin(&mut s.bq_d, xi.sin());
                }
            }
            self.flip = !self.flip;

            let ground = [vib_dry[0] - x[0], vib_dry[1] - x[1]];
            if inputgain > 1.0 {
                x[0] *= inputgain;
                x[1] *= inputgain;
            }
            for ch in 0..2 {
                let mut apply_soften = highs[ch].abs() * 1.57079633;
                if apply_soften > 1.57079633 {
                    apply_soften = 1.57079633;
                }
                apply_soften = 1.0 - apply_soften.cos();
                if highs[ch] > 0.0 {
                    x[ch] -= apply_soften;
                }
                if highs[ch] < 0.0 {
                    x[ch] += apply_soften;
                }
            }
            for ch in 0..2 {
                let s = &mut self.s[ch];
                let suppress = (1.0 - x[ch].abs()) * 0.00013;
                if s.head_bump_a > suppress {
                    s.head_bump_a -= suppress;
                }
                if s.head_bump_a < -suppress {
                    s.head_bump_a += suppress;
                }
                if s.head_bump_b > suppress {
                    s.head_bump_b -= suppress;
                }
                if s.head_bump_b < -suppress {
                    s.head_bump_b += suppress;
                }
            }
            for ch in 0..2 {
                let s = &mut self.s[ch];
                x[ch] += (s.head_bump_a + s.head_bump_b) * head_bump_control;
            }
            for ch in 0..2 {
                let mut xi = x[ch].clamp(-1.0, 1.0);
                let mojo = xi.abs().powf(0.25);
                if mojo > 0.0 {
                    xi = (xi * mojo * PI * 0.5).sin() / mojo;
                }
                x[ch] = xi;
            }
            for ch in 0..2 {
                x[ch] += ground[ch];
                if outputgain != 1.0 {
                    x[ch] *= outputgain;
                }
            }
            for ch in 0..2 {
                let s = &mut self.s[ch];
                let mut xi = x[ch];
                if s.last >= refclip {
                    if xi < refclip {
                        s.last = (refclip * softness) + (xi * (1.0 - softness));
                    } else {
                        s.last = refclip;
                    }
                }
                if s.last <= -refclip {
                    if xi > -refclip {
                        s.last = (-refclip * softness) + (xi * (1.0 - softness));
                    } else {
                        s.last = -refclip;
                    }
                }
                if xi > refclip {
                    if s.last < refclip {
                        xi = (refclip * softness) + (s.last * (1.0 - softness));
                    } else {
                        xi = refclip;
                    }
                }
                if xi < -refclip {
                    if s.last > -refclip {
                        xi = (-refclip * softness) + (s.last * (1.0 - softness));
                    } else {
                        xi = -refclip;
                    }
                }
                s.last = xi;
                x[ch] = xi;
            }
            for ch in 0..2 {
                let mut xi = x[ch];
                if xi > refclip {
                    xi = refclip;
                }
                if xi < -refclip {
                    xi = -refclip;
                }
                if wet != 1.0 {
                    xi = (xi * wet) + (dry[ch] * (1.0 - wet));
                }
                x[ch] = dither32(xi, &mut self.fpd[ch]);
            }
            *sl = x[0] as f32;
            *sr = x[1] as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_never_exceeds_the_iron_bar() {
        let mut t = ToTape6::new(48000.0, &mut CRand::new(3));
        t.set_param(0, 1.0); // +12 dB in
        for k in 0..200 {
            let mut l = [0.0f32; 4];
            let mut r = [0.0f32; 4];
            for i in 0..4 {
                l[i] = ((k * 4 + i) as f32 * 0.05).sin() * 2.0;
                r[i] = -l[i];
            }
            t.process(&mut l, &mut r);
            for v in l.iter().chain(r.iter()) {
                assert!(v.abs() <= 0.990001, "{}", v);
            }
        }
    }
}
