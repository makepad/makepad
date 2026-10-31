// Ported from Capacitor (CapacitorProc.cpp, Capacitor.cpp) of the MIT-licensed effects collection by Chris Johnson.
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

//! Six one-pole highpass/lowpass pairs per channel run three at a time in
//! a rotating "gearbox", with chased (smoothed) controls.

use super::{denorm, dither32, pf, CRand, Sub};
use crate::kit::core::SlotDef;

pub const PARAMS: &[SlotDef] = &[pf("Lowpass", 1.0), pf("Highpass", 0.0), pf("Dry/Wet", 1.0)];

#[derive(Default, Clone, Copy)]
struct Poles {
    hp: [f64; 6], // A..F
    lp: [f64; 6],
}

impl Poles {
    #[inline]
    fn pole(&mut self, k: usize, x: &mut f64, hpa: f64, ihp: f64, lpa: f64, ilp: f64) {
        self.hp[k] = (self.hp[k] * ihp) + (*x * hpa);
        *x -= self.hp[k];
        self.lp[k] = (self.lp[k] * ilp) + (*x * lpa);
        *x = self.lp[k];
    }
}

/// Pole sets per gearbox position (A always, then B/C, then D/E/F).
const GEARS: [[usize; 3]; 6] = [[0, 1, 3], [0, 2, 4], [0, 1, 5], [0, 2, 3], [0, 1, 4], [0, 2, 5]];

pub struct Capacitor {
    p: [f32; 3],
    l: Poles,
    r: Poles,
    count: i32,
    lowpass_chase: f64,
    highpass_chase: f64,
    wet_chase: f64,
    lowpass_amount: f64,
    highpass_amount: f64,
    wet: f64,
    last_lowpass: f64,
    last_highpass: f64,
    last_wet: f64,
    fpd_l: u32,
    fpd_r: u32,
}

impl Capacitor {
    pub fn new(_sr: f64, rng: &mut CRand) -> Box<Capacitor> {
        let fpd_l = rng.fpd();
        let fpd_r = rng.fpd();
        Box::new(Capacitor {
            p: [1.0, 0.0, 1.0],
            l: Poles::default(),
            r: Poles::default(),
            count: 0,
            lowpass_chase: 0.0,
            highpass_chase: 0.0,
            wet_chase: 0.0,
            lowpass_amount: 1.0,
            highpass_amount: 0.0,
            wet: 1.0,
            last_lowpass: 1000.0,
            last_highpass: 1000.0,
            last_wet: 1000.0,
            fpd_l,
            fpd_r,
        })
    }
}

impl Sub for Capacitor {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let a = self.p[0] as f64;
        let b = self.p[1] as f64;
        self.lowpass_chase = a.powf(2.0);
        self.highpass_chase = b.powf(2.0);
        self.wet_chase = self.p[2] as f64;
        let lowpass_speed = 300.0 / ((self.last_lowpass - self.lowpass_chase).abs() + 1.0);
        let highpass_speed = 300.0 / ((self.last_highpass - self.highpass_chase).abs() + 1.0);
        let wet_speed = 300.0 / ((self.last_wet - self.wet_chase).abs() + 1.0);
        self.last_lowpass = self.lowpass_chase;
        self.last_highpass = self.highpass_chase;
        self.last_wet = self.wet_chase;

        for (sl, sr) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let mut in_l = denorm(*sl as f64, self.fpd_l);
            let mut in_r = denorm(*sr as f64, self.fpd_r);
            let dry_l = in_l as f32;
            let dry_r = in_r as f32;

            self.lowpass_amount = ((self.lowpass_amount * lowpass_speed) + self.lowpass_chase) / (lowpass_speed + 1.0);
            let inv_lowpass = 1.0 - self.lowpass_amount;
            self.highpass_amount = ((self.highpass_amount * highpass_speed) + self.highpass_chase) / (highpass_speed + 1.0);
            let inv_highpass = 1.0 - self.highpass_amount;
            self.wet = ((self.wet * wet_speed) + self.wet_chase) / (wet_speed + 1.0);
            let dry = 1.0 - self.wet;

            self.count += 1;
            if self.count > 5 {
                self.count = 0;
            }
            let (hpa, lpa) = (self.highpass_amount, self.lowpass_amount);
            for k in GEARS[self.count as usize] {
                self.l.pole(k, &mut in_l, hpa, inv_highpass, lpa, inv_lowpass);
            }
            for k in GEARS[self.count as usize] {
                self.r.pole(k, &mut in_r, hpa, inv_highpass, lpa, inv_lowpass);
            }

            in_l = (dry_l as f64 * dry) + (in_l * self.wet);
            in_r = (dry_r as f64 * dry) + (in_r * self.wet);

            in_l = dither32(in_l, &mut self.fpd_l);
            in_r = dither32(in_r, &mut self.fpd_r);
            *sl = in_l as f32;
            *sr = in_r as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_a_bypass() {
        // lowpass 1, highpass 0: every pole passes the input unchanged
        let mut c = Capacitor::new(48000.0, &mut CRand::new(1));
        let mut l = [0.5f32, -0.25, 0.125, 0.75];
        let mut r = [0.1f32, 0.2, -0.3, 0.4];
        let (l0, r0) = (l, r);
        c.process(&mut l, &mut r);
        for i in 0..4 {
            assert!((l[i] - l0[i]).abs() <= l0[i].abs() * 1e-6);
            assert!((r[i] - r0[i]).abs() <= r0[i].abs() * 1e-6);
        }
    }
}
