// Ported from NCSeventeen (NCSeventeenProc.cpp, NCSeventeen.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Two-band (and full-band) sine saturation with adaptive second-harmonic
//! and level correction, into an iron-bar clip.

use super::{denorm, dither32, pf, CRand, Sub};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[pf("LOUDER", 0.0), pf("Output", 1.0)];

#[derive(Clone, Copy, Default)]
struct Side {
    iir_a: f64,
    iir_b: f64,
    basslev: f64,
    treblev: f64,
    cheblev: f64,
}

/// One saturation stage with its adaptive level (the block upstream repeats
/// per band and channel).
#[inline]
fn stage(x: f64, lev: &mut f64, scaleback: f64, maxfeed: f64, add_buss: f64, limiting: f64) -> f64 {
    let mut in_p2 = x * x;
    if in_p2 > 1.0 {
        in_p2 = 1.0;
    }
    if in_p2 < -1.0 {
        in_p2 = -1.0;
    }
    let mut chebyshev = 2.0 * in_p2;
    chebyshev *= *lev;
    if *lev > 0.0 {
        *lev -= scaleback;
    }
    if *lev < 0.0 {
        *lev += scaleback;
    }
    let mut br = x.abs();
    if br > 1.57079633 {
        br = 1.57079633;
    }
    br = br.sin();
    let dist = if x > 0.0 { br } else { -br };
    let mut minus = x - dist;
    let mut plus = x + dist;
    if minus > maxfeed {
        minus = maxfeed;
    }
    if plus > maxfeed {
        plus = maxfeed;
    }
    if plus < -maxfeed {
        plus = -maxfeed;
    }
    if minus < -maxfeed {
        minus = -maxfeed;
    }
    if x > dist {
        *lev += minus * add_buss;
    }
    if x < -dist {
        *lev -= plus * add_buss;
    }
    if *lev > 1.0 {
        *lev = 1.0;
    }
    if *lev < -1.0 {
        *lev = -1.0;
    }
    let mut br = x.abs();
    if br > 1.57079633 {
        br = 1.57079633;
    }
    br = br.sin();
    let mut y = if x > 0.0 { br } else { -br };
    y /= 1.0 + (*lev * limiting).abs();
    y + chebyshev
}

pub struct NcSeventeen {
    p: [f32; 2],
    sr: f64,
    s: [Side; 2],
    flip: bool,
    fpd: [u32; 2],
}

impl NcSeventeen {
    pub fn new(sr: f64, rng: &mut CRand) -> Box<NcSeventeen> {
        let l = rng.fpd();
        let r = rng.fpd();
        Box::new(NcSeventeen { p: [0.0, 1.0], sr, s: [Side::default(); 2], flip: false, fpd: [l, r] })
    }
}

impl Sub for NcSeventeen {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let mut overallscale = 1.0f64;
        overallscale /= 44100.0;
        overallscale *= self.sr;
        let iir_scaleback = 0.0004716 / overallscale;
        let bass_scaleback = 0.0002364 / overallscale;
        let treble_scaleback = 0.0005484 / overallscale;
        let add_bass_buss = 0.000243 / overallscale;
        let add_treb_buss = 0.000407 / overallscale;
        let add_short_buss = 0.000326 / overallscale;
        let limiting_bass = 0.39;
        let limiting_treb = 0.6;
        let limiting = 0.36;
        let maxfeed_bass = 0.972;
        let maxfeed_treb = 0.972;
        let maxfeed = 0.975;
        let gain = 10f64.powf((self.p[0] as f64 * 24.0) / 20.0);
        let outlevel = self.p[1] as f64;

        for (sl, sr) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let mut x = [denorm(*sl as f64, self.fpd[0]), denorm(*sr as f64, self.fpd[1])];
            let mut low = [0.0f64; 2];
            for ch in 0..2 {
                x[ch] *= gain;
                let s = &mut self.s[ch];
                if self.flip {
                    s.iir_a = (s.iir_a * 0.9) + (x[ch] * 0.1);
                    low[ch] = s.iir_a;
                } else {
                    s.iir_b = (s.iir_b * 0.9) + (x[ch] * 0.1);
                    low[ch] = s.iir_b;
                }
            }
            let high = [x[0] - low[0], x[1] - low[1]];
            self.flip = !self.flip;
            for ch in 0..2 {
                let s = &mut self.s[ch];
                let lo = stage(low[ch], &mut s.basslev, bass_scaleback, maxfeed_bass, add_bass_buss, limiting_bass);
                let hi = stage(high[ch], &mut s.treblev, treble_scaleback, maxfeed_treb, add_treb_buss, limiting_treb);
                let mut y = stage(lo + hi, &mut s.cheblev, iir_scaleback, maxfeed, add_short_buss, limiting);
                if outlevel < 1.0 {
                    y *= outlevel;
                }
                if y > 0.95 {
                    y = 0.95;
                }
                if y < -0.95 {
                    y = -0.95;
                }
                x[ch] = dither32(y, &mut self.fpd[ch]);
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
    fn iron_bar_holds() {
        let mut n = NcSeventeen::new(48000.0, &mut CRand::new(1));
        n.set_param(0, 1.0);
        for k in 0..100 {
            let mut l = [0.0f32; 4];
            let mut r = [0.0f32; 4];
            for i in 0..4 {
                l[i] = ((k * 4 + i) as f32 * 0.03).sin();
                r[i] = l[i] * 0.5;
            }
            n.process(&mut l, &mut r);
            assert!(l.iter().chain(r.iter()).all(|v| v.abs() <= 0.9500001));
        }
    }
}
