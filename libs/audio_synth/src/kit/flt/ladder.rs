//! Ladders (FILTERS.md 5.4, 5.7): the legacy four-pole ladder and the three
//! vintage ladder models (Runge-Kutta, two thermal-voltage models).

use super::approx::{exp_r, tanh_r};
use super::{hz_clamped, FilterUnit, NC};
use crate::kit::core::note_hz;
use std::f32::consts::PI;

/// [b, q, c0].
pub(crate) fn make_legacy(n: f32, r: f32, fs: f32) -> [f32; NC] {
    let g = (note_hz(n) / fs).clamp(0.0, 0.187);
    let b = 1.0 - (-2.0 * PI * g).exp();
    let q = (2.15 * r.clamp(0.0, 1.0)).min(0.5 / (b * b * b * b));
    [b, q, 3.0 / (3.0 - q), 0.0, 0.0, 0.0, 0.0, 0.0]
}

/// [omega_k, rho, gc].
pub(crate) fn make_vintage1(comp: bool, n: f32, r: f32, fs: f32) -> [f32; NC] {
    let f = hz_clamped(n, fs);
    [2.0 * PI * f, 4.5 * r.clamp(0.0, 1.0), if comp { 0.666 } else { 0.0 }, 0.0, 0.0, 0.0, 0.0, 0.0]
}

/// [fc, rho, gc].
pub(crate) fn make_vintage2(comp: bool, n: f32, r: f32, fs: f32) -> [f32; NC] {
    let f = hz_clamped(n, fs);
    let co = (f - fs / 3.0).max(0.0) * 0.1 / fs;
    let gt = if comp { 0.05 } else { 0.0 };
    let rho = r.clamp(0.0, 0.9925).min(0.994 - co - gt);
    [f / fs, rho, if comp { 0.5 } else { 0.0 }, 0.0, 0.0, 0.0, 0.0, 0.0]
}

/// [gres, gp, gc].
pub(crate) fn make_vintage3(comp: bool, n: f32, r: f32, fs: f32) -> [f32; NC] {
    let w = 2.0 * PI * hz_clamped(n, fs) / fs;
    let rho = r.clamp(0.0, 1.0);
    let gres = 4.5 * rho * (1.0029 + w * (0.0526 + w * (-0.0926 + 0.0218 * w)));
    let gp = w * (0.9892 + w * (-0.4342 + w * (0.1381 - 0.0202 * w)));
    [gres, gp, if comp { -0.5 } else { 0.0 }, 0.0, 0.0, 0.0, 0.0, 0.0]
}

#[inline]
fn sc8(v: f32) -> f32 {
    let v = v.clamp(-12.0, 12.0);
    v - 0.00028935185 * v * v * v
}

#[inline]
fn sat3(v: f32) -> f32 {
    let u = (v / 3.0).clamp(-1.0, 1.0);
    3.0 * (u - u * u * u / 3.0)
}

impl FilterUnit {
    /// s: y1..y4, y4_prev.
    #[inline]
    pub(crate) fn tick_legacy(&mut self, x: f32, out: u8) -> f32 {
        self.step(1.0);
        let [b, q, c0, ..] = self.c;
        let s = &mut self.s;
        s[0] = sc8(s[0] + b * (c0 * x - q * (s[3] + s[4]) - s[0]));
        s[1] += b * (s[0] - s[1]);
        s[2] += b * (s[1] - s[2]);
        s[4] = s[3];
        s[3] += b * (s[2] - s[3]);
        let y = s[out as usize];
        y
    }

    /// s: y1..y4. Four RK4 sub-steps per sample, input zero-stuffed.
    pub(crate) fn tick_vintage1(&mut self, x: f32, fs: f32) -> f32 {
        let h = 1.0 / (4.0 * fs);
        let mut o = [0.0f32; 4];
        for (i, oi) in o.iter_mut().enumerate() {
            self.step(0.25);
            let [wk, rho, gc, ..] = self.c;
            let u = if i == 0 { x } else { 0.0 };
            let d = |y: [f32; 4]| {
                let s1 = sat3(y[0]);
                let s2 = sat3(y[1]);
                let s3 = sat3(y[2]);
                [
                    wk * (sat3(u - rho * (y[3] - gc * u)) - s1),
                    wk * (s1 - s2),
                    wk * (s2 - s3),
                    wk * (s3 - sat3(y[3])),
                ]
            };
            let y0 = [self.s[0], self.s[1], self.s[2], self.s[3]];
            let add = |a: [f32; 4], k: [f32; 4], m: f32| [a[0] + m * k[0], a[1] + m * k[1], a[2] + m * k[2], a[3] + m * k[3]];
            let k1 = d(y0);
            let k2 = d(add(y0, k1, h / 2.0));
            let k3 = d(add(y0, k2, h / 2.0));
            let k4 = d(add(y0, k3, h));
            for j in 0..4 {
                self.s[j] = y0[j] + h / 6.0 * (k1[j] + 2.0 * k2[j] + 2.0 * k3[j] + k4[j]);
            }
            *oi = self.s[3];
        }
        1.5 * (-0.0636844 * o[0] + 0.57315917 * o[2] + o[3])
    }

    /// s: d0..d4, t0..t2, z5. Two iterations per sample.
    pub(crate) fn tick_vintage2(&mut self, x: f32) -> f32 {
        for _ in 0..2 {
            let fc = self.c[0];
            let fr = fc / 2.0;
            let fcr = 1.8730 * fc * fc * fc + 0.4995 * fc * fc - 0.6490 * fc + 0.9988;
            let acr = -3.9364 * fc * fc + 1.8409 * fc + 0.9968;
            let tune = (1.0 - exp_r(-2.0 * PI * fr * fcr)) * 70.0;
            // Resonance is read before the step, gc (constant) after it.
            let rho = self.c[1];
            self.step(0.5);
            let gc = self.c[2];
            let s = &mut self.s;
            let r4 = 4.0 * rho * acr;
            let u = x - r4 * (s[8] - gc * x);
            let s0 = s[0] + tune * (tanh_r(u / 70.0) - s[5]);
            s[0] = s0;
            s[5] = tanh_r(s0 / 70.0);
            let s1 = s[1] + tune * (s[5] - s[6]);
            s[1] = s1;
            s[6] = tanh_r(s1 / 70.0);
            let s2 = s[2] + tune * (s[6] - s[7]);
            s[2] = s2;
            s[7] = tanh_r(s2 / 70.0);
            let s3 = s[3] + tune * (s[7] - tanh_r(s[3] / 70.0));
            s[3] = s3;
            s[8] = 0.5 * (s3 + s[4]);
            s[4] = s3;
        }
        self.s[8]
    }

    /// s: z, in_prev[4], out_prev[4].
    #[inline]
    pub(crate) fn tick_vintage3(&mut self, x: f32) -> f32 {
        let [gres, gp, gc, ..] = self.c;
        let s = &mut self.s;
        let fb = gres * (s[0] + gc * 4.0 * x);
        let mut v = 70.0 * tanh_r((4.0 * x - fb) / 70.0);
        for j in 0..4 {
            let m = (0.3 / 1.3) * s[1 + j] + (1.0 / 1.3) * v;
            s[1 + j] = v;
            let o = gp * (m - s[5 + j]) + s[5 + j];
            s[5 + j] = o;
            v = o;
        }
        s[0] = v;
        self.step(1.0);
        v * (0.5 / 4.0)
    }
}
