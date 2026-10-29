// Ported from Galactic (GalacticProc.cpp, Galactic.cpp) of the MIT-licensed
// effects collection by Chris Johnson.
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

//! A big, dark, vibrato-predelayed 3x4 feedback-matrix reverb.

use super::{dither32, denorm, CRand, Sub};
use crate::kit::core::{SlotDef, SlotUnit};

pub const PARAMS: &[SlotDef] = &[
    SlotDef::float("Replace", 0.0, 1.0, 0.5, SlotUnit::None),
    SlotDef::float("Brightness", 0.0, 1.0, 0.5, SlotUnit::None),
    SlotDef::float("Detune", 0.0, 1.0, 0.5, SlotUnit::None),
    SlotDef::float("Bigness", 0.0, 1.0, 1.0, SlotUnit::None),
    SlotDef::float("Dry/Wet", 0.0, 1.0, 1.0, SlotUnit::None),
];

/// One channel's delay network.
struct Chan {
    iir_a: f64,
    iir_b: f64,
    a_i: Vec<f64>,
    a_j: Vec<f64>,
    a_k: Vec<f64>,
    a_l: Vec<f64>,
    a_a: Vec<f64>,
    a_b: Vec<f64>,
    a_c: Vec<f64>,
    a_d: Vec<f64>,
    a_e: Vec<f64>,
    a_f: Vec<f64>,
    a_g: Vec<f64>,
    a_h: Vec<f64>,
    a_m: Vec<f64>,
    feedback: [f64; 4],
    last_ref: [f64; 7],
}

impl Chan {
    fn new() -> Chan {
        Chan {
            iir_a: 0.0,
            iir_b: 0.0,
            a_i: vec![0.0; 6480],
            a_j: vec![0.0; 3660],
            a_k: vec![0.0; 1720],
            a_l: vec![0.0; 680],
            a_a: vec![0.0; 9700],
            a_b: vec![0.0; 6000],
            a_c: vec![0.0; 2320],
            a_d: vec![0.0; 940],
            a_e: vec![0.0; 15220],
            a_f: vec![0.0; 8460],
            a_g: vec![0.0; 4540],
            a_h: vec![0.0; 3200],
            a_m: vec![0.0; 3111],
            feedback: [0.0; 4],
            last_ref: [0.0; 7],
        }
    }
}

pub struct Galactic {
    p: [f32; 5],
    sr: f64,
    l: Chan,
    r: Chan,
    count: [usize; 13], // A..M
    cycle: i32,
    vib_m: f64,
    oldfpd: f64,
    fpd_l: u32,
    fpd_r: u32,
}

// count / delay indices
const A: usize = 0;
const B: usize = 1;
const C: usize = 2;
const D: usize = 3;
const E: usize = 4;
const F: usize = 5;
const G: usize = 6;
const H: usize = 7;
const I: usize = 8;
const J: usize = 9;
const K: usize = 10;
const L: usize = 11;
const M: usize = 12;

#[inline]
fn tap(buf: &[f64], count: usize, delay: usize) -> f64 {
    buf[if count > delay { count - (delay + 1) } else { count }]
}

#[inline]
fn advance(count: &mut usize, delay: usize) {
    *count += 1;
    if *count > delay {
        *count = 0;
    }
}

impl Galactic {
    pub fn new(sr: f64, rng: &mut CRand) -> Box<Galactic> {
        let fpd_l = rng.fpd();
        let fpd_r = rng.fpd();
        Box::new(Galactic {
            p: [0.5, 0.5, 0.5, 1.0, 1.0],
            sr,
            l: Chan::new(),
            r: Chan::new(),
            count: [1; 13],
            cycle: 0,
            vib_m: 3.0,
            oldfpd: 429496.7295,
            fpd_l,
            fpd_r,
        })
    }
}

impl Sub for Galactic {
    fn set_param(&mut self, i: usize, v: f32) {
        if i < self.p.len() {
            self.p[i] = v;
        }
    }

    fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let [pa, pb, pc, pd, pe] = self.p.map(|v| v as f64);
        let mut overallscale = 1.0f64;
        overallscale /= 44100.0;
        overallscale *= self.sr;

        let mut cycle_end = overallscale.floor() as i32;
        if cycle_end < 1 {
            cycle_end = 1;
        }
        if cycle_end > 4 {
            cycle_end = 4;
        }
        if self.cycle > cycle_end - 1 {
            self.cycle = cycle_end - 1;
        }

        let regen = 0.0625 + ((1.0 - pa) * 0.0625);
        let attenuate = (1.0 - (regen / 0.125)) * 1.333;
        let lowpass = (1.00001 - (1.0 - pb)).powf(2.0) / overallscale.sqrt();
        let drift = pc.powi(3) * 0.001;
        let size = (pd * 1.77) + 0.1;
        let wet = 1.0 - (1.0 - pe).powi(3);

        let mut delay = [0usize; 13];
        delay[I] = (3407.0 * size) as usize;
        delay[J] = (1823.0 * size) as usize;
        delay[K] = (859.0 * size) as usize;
        delay[L] = (331.0 * size) as usize;
        delay[A] = (4801.0 * size) as usize;
        delay[B] = (2909.0 * size) as usize;
        delay[C] = (1153.0 * size) as usize;
        delay[D] = (461.0 * size) as usize;
        delay[E] = (7607.0 * size) as usize;
        delay[F] = (4217.0 * size) as usize;
        delay[G] = (2269.0 * size) as usize;
        delay[H] = (1597.0 * size) as usize;
        delay[M] = 256;

        let pi = std::f64::consts::PI;
        for (sl, sr) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let mut in_l = denorm(*sl as f64, self.fpd_l);
            let mut in_r = denorm(*sr as f64, self.fpd_r);
            let dry_l = in_l;
            let dry_r = in_r;

            self.vib_m += self.oldfpd * drift;
            if self.vib_m > pi * 2.0 {
                self.vib_m = 0.0;
                self.oldfpd = 0.4294967295 + (self.fpd_l as f64 * 0.0000000000618);
            }

            let cm = self.count[M];
            self.l.a_m[cm] = in_l * attenuate;
            self.r.a_m[cm] = in_r * attenuate;
            advance(&mut self.count[M], delay[M]);
            let cm = self.count[M];
            let dm = delay[M];

            let offset_ml = ((self.vib_m).sin() + 1.0) * 127.0;
            let offset_mr = ((self.vib_m + (pi / 2.0)).sin() + 1.0) * 127.0;
            let working_ml = (cm as f64 + offset_ml) as usize;
            let working_mr = (cm as f64 + offset_mr) as usize;
            let wrap = |w: usize| if w > dm { w - (dm + 1) } else { w };
            let frac_l = offset_ml - offset_ml.floor();
            let frac_r = offset_mr - offset_mr.floor();
            let mut interpol_ml = self.l.a_m[wrap(working_ml)] * (1.0 - frac_l);
            interpol_ml += self.l.a_m[wrap(working_ml + 1)] * frac_l;
            let mut interpol_mr = self.r.a_m[wrap(working_mr)] * (1.0 - frac_r);
            interpol_mr += self.r.a_m[wrap(working_mr + 1)] * frac_r;
            in_l = interpol_ml;
            in_r = interpol_mr;

            self.l.iir_a = (self.l.iir_a * (1.0 - lowpass)) + (in_l * lowpass);
            in_l = self.l.iir_a;
            self.r.iir_a = (self.r.iir_a * (1.0 - lowpass)) + (in_r * lowpass);
            in_r = self.r.iir_a;

            self.cycle += 1;
            if self.cycle == cycle_end {
                let c = &mut self.count;
                let (l, r) = (&mut self.l, &mut self.r);
                l.a_i[c[I]] = in_l + (r.feedback[0] * regen);
                l.a_j[c[J]] = in_l + (r.feedback[1] * regen);
                l.a_k[c[K]] = in_l + (r.feedback[2] * regen);
                l.a_l[c[L]] = in_l + (r.feedback[3] * regen);
                r.a_i[c[I]] = in_r + (l.feedback[0] * regen);
                r.a_j[c[J]] = in_r + (l.feedback[1] * regen);
                r.a_k[c[K]] = in_r + (l.feedback[2] * regen);
                r.a_l[c[L]] = in_r + (l.feedback[3] * regen);

                for x in [I, J, K, L] {
                    advance(&mut c[x], delay[x]);
                }

                let out_il = tap(&l.a_i, c[I], delay[I]);
                let out_jl = tap(&l.a_j, c[J], delay[J]);
                let out_kl = tap(&l.a_k, c[K], delay[K]);
                let out_ll = tap(&l.a_l, c[L], delay[L]);
                let out_ir = tap(&r.a_i, c[I], delay[I]);
                let out_jr = tap(&r.a_j, c[J], delay[J]);
                let out_kr = tap(&r.a_k, c[K], delay[K]);
                let out_lr = tap(&r.a_l, c[L], delay[L]);

                l.a_a[c[A]] = out_il - (out_jl + out_kl + out_ll);
                l.a_b[c[B]] = out_jl - (out_il + out_kl + out_ll);
                l.a_c[c[C]] = out_kl - (out_il + out_jl + out_ll);
                l.a_d[c[D]] = out_ll - (out_il + out_jl + out_kl);
                r.a_a[c[A]] = out_ir - (out_jr + out_kr + out_lr);
                r.a_b[c[B]] = out_jr - (out_ir + out_kr + out_lr);
                r.a_c[c[C]] = out_kr - (out_ir + out_jr + out_lr);
                r.a_d[c[D]] = out_lr - (out_ir + out_jr + out_kr);

                for x in [A, B, C, D] {
                    advance(&mut c[x], delay[x]);
                }

                let out_al = tap(&l.a_a, c[A], delay[A]);
                let out_bl = tap(&l.a_b, c[B], delay[B]);
                let out_cl = tap(&l.a_c, c[C], delay[C]);
                let out_dl = tap(&l.a_d, c[D], delay[D]);
                let out_ar = tap(&r.a_a, c[A], delay[A]);
                let out_br = tap(&r.a_b, c[B], delay[B]);
                let out_cr = tap(&r.a_c, c[C], delay[C]);
                let out_dr = tap(&r.a_d, c[D], delay[D]);

                l.a_e[c[E]] = out_al - (out_bl + out_cl + out_dl);
                l.a_f[c[F]] = out_bl - (out_al + out_cl + out_dl);
                l.a_g[c[G]] = out_cl - (out_al + out_bl + out_dl);
                l.a_h[c[H]] = out_dl - (out_al + out_bl + out_cl);
                r.a_e[c[E]] = out_ar - (out_br + out_cr + out_dr);
                r.a_f[c[F]] = out_br - (out_ar + out_cr + out_dr);
                r.a_g[c[G]] = out_cr - (out_ar + out_br + out_dr);
                r.a_h[c[H]] = out_dr - (out_ar + out_br + out_cr);

                for x in [E, F, G, H] {
                    advance(&mut c[x], delay[x]);
                }

                let out_el = tap(&l.a_e, c[E], delay[E]);
                let out_fl = tap(&l.a_f, c[F], delay[F]);
                let out_gl = tap(&l.a_g, c[G], delay[G]);
                let out_hl = tap(&l.a_h, c[H], delay[H]);
                let out_er = tap(&r.a_e, c[E], delay[E]);
                let out_fr = tap(&r.a_f, c[F], delay[F]);
                let out_gr = tap(&r.a_g, c[G], delay[G]);
                let out_hr = tap(&r.a_h, c[H], delay[H]);

                l.feedback[0] = out_el - (out_fl + out_gl + out_hl);
                l.feedback[1] = out_fl - (out_el + out_gl + out_hl);
                l.feedback[2] = out_gl - (out_el + out_fl + out_hl);
                l.feedback[3] = out_hl - (out_el + out_fl + out_gl);
                r.feedback[0] = out_er - (out_fr + out_gr + out_hr);
                r.feedback[1] = out_fr - (out_er + out_gr + out_hr);
                r.feedback[2] = out_gr - (out_er + out_fr + out_hr);
                r.feedback[3] = out_hr - (out_er + out_fr + out_gr);

                in_l = (out_el + out_fl + out_gl + out_hl) / 8.0;
                in_r = (out_er + out_fr + out_gr + out_hr) / 8.0;

                for (ch, x) in [(&mut *l, in_l), (&mut *r, in_r)] {
                    let lr = &mut ch.last_ref;
                    if cycle_end == 4 {
                        lr[0] = lr[4];
                        lr[2] = (lr[0] + x) / 2.0;
                        lr[1] = (lr[0] + lr[2]) / 2.0;
                        lr[3] = (lr[2] + x) / 2.0;
                        lr[4] = x;
                    }
                    if cycle_end == 3 {
                        lr[0] = lr[3];
                        lr[2] = (lr[0] + lr[0] + x) / 3.0;
                        lr[1] = (lr[0] + x + x) / 3.0;
                        lr[3] = x;
                    }
                    if cycle_end == 2 {
                        lr[0] = lr[2];
                        lr[1] = (lr[0] + x) / 2.0;
                        lr[2] = x;
                    }
                    if cycle_end == 1 {
                        lr[0] = x;
                    }
                }
                self.cycle = 0;
                in_l = self.l.last_ref[0];
                in_r = self.r.last_ref[0];
            } else {
                in_l = self.l.last_ref[self.cycle as usize];
                in_r = self.r.last_ref[self.cycle as usize];
            }

            self.l.iir_b = (self.l.iir_b * (1.0 - lowpass)) + (in_l * lowpass);
            in_l = self.l.iir_b;
            self.r.iir_b = (self.r.iir_b * (1.0 - lowpass)) + (in_r * lowpass);
            in_r = self.r.iir_b;

            if wet < 1.0 {
                in_l = (in_l * wet) + (dry_l * (1.0 - wet));
                in_r = (in_r * wet) + (dry_r * (1.0 - wet));
            }

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
    fn dry_setting_is_near_unity() {
        // Dry/Wet 0: wet = 0, the output is the input plus the 32-bit dither
        let mut rng = CRand::new(1);
        let mut g = Galactic::new(48000.0, &mut rng);
        g.set_param(4, 0.0);
        let mut l = [0.5f32, -0.25, 0.125, 0.75];
        let mut r = [0.1f32, 0.2, -0.3, 0.4];
        let (l0, r0) = (l, r);
        g.process(&mut l, &mut r);
        for i in 0..4 {
            assert!((l[i] - l0[i]).abs() <= l0[i].abs() * 1e-6);
            assert!((r[i] - r0[i]).abs() <= r0[i].abs() * 1e-6);
        }
    }
}
