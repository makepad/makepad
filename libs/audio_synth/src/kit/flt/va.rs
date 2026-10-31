//! Zero-delay-feedback analog models (FILTERS.md 5.10, 5.11, 5.15): the Sallen-Key
//! low/high pass, the diode ladder and the trapezoidal fast SVF.

use super::approx::{tan_r, tanh_r};
use super::{hz_clamped, FilterUnit, NC};
use crate::kit::core::note_hz;
use std::f32::consts::PI;

/// [G, lambda, eta, alpha, k, sigma, beta].
pub(crate) fn make_sallen_key(hp: bool, sub: u8, n: f32, r: f32, fs: f32) -> [f32; NC] {
    let g = tan_r(PI * hz_clamped(n, fs) / fs);
    let gg = g / (1.0 + g);
    let k = (1.96 * r).clamp(0.01, 1.96);
    let sigma = sub as f32;
    let (l, e) = if hp { (1.0 / (1.0 + g), -gg / (1.0 + g)) } else { ((k - k * gg) / (1.0 + g), -1.0 / (1.0 + g)) };
    let a = 1.0 / (1.0 - k * gg + k * gg * gg);
    [gg, l, e, a, k, sigma, sigma.min(1.0), 0.0]
}

/// [g, G1, G2, G3, G4, Gamma, G, kappa].
pub(crate) fn make_diode(n: f32, r: f32, fs: f32) -> [f32; NC] {
    let g = tan_r(PI * hz_clamped(n, fs) / fs);
    let g4 = 0.5 * g / (1.0 + g);
    let g3 = 0.5 * g / (1.0 + g - 0.5 * g * g4);
    let g2 = 0.5 * g / (1.0 + g - 0.5 * g * g3);
    let g1 = g / (1.0 + g - g * g2);
    [g, g1, g2, g3, g4, g1 * g2 * g3 * g4, g / (1.0 + g), (16.0 * r).clamp(0.0, 16.0)]
}

/// [a1, a2, a3, m0, m1, m2].
pub(crate) fn make_fastsvf(sub: u8, n: f32, r: f32, fs: f32) -> [f32; NC] {
    let g = tan_r(PI * (note_hz(n) / fs).clamp(0.0, 0.499));
    let rho = r.clamp(0.0, 0.99);
    let mut k = 2.0 - 2.0 * rho;
    let a: f32 = if super::QUIRK_FAST_SVF_AMPLITUDE_ZERO { 0.0 } else { 1.0 };
    let ab = a.max(0.001);
    if sub == 6 {
        k /= ab;
    }
    let (m0, m1, m2) = match sub {
        0 => (0.0, 0.0, 1.0),
        1 => (1.0, -k, -1.0),
        2 => (0.0, 1.0, 0.0),
        3 => (1.0, -k, 0.0),
        4 => (1.0, -k, -2.0),
        5 => (1.0, -2.0 * k, 0.0),
        6 => (1.0, k * (ab * ab - 1.0), 0.0),
        7 => (1.0, k * (a - 1.0), a * a - 1.0),
        _ => (a * a, a * k * (1.0 - a), 1.0 - a * a),
    };
    let a1 = 1.0 / (1.0 + g * (g + k));
    let a2 = g * a1;
    [a1, a2, g * a2, m0, m1, m2, 0.0, 0.0]
}

#[inline]
fn lpf(z: &mut f32, u: f32, g: f32) -> f32 {
    let v = (u - *z) * g;
    let o = v + *z;
    *z = v + o;
    o
}

impl FilterUnit {
    /// s: zL, zH, z2.
    #[inline]
    pub(crate) fn tick_sallen_key(&mut self, x: f32, hp: bool) -> f32 {
        self.step(1.0);
        let [gg, l, e, a, k, sigma, beta, _] = self.c;
        let s = &mut self.s;
        let y = if !hp {
            let lo = lpf(&mut s[0], x, gg);
            let st = l * s[2] + e * s[1];
            let mut u = a * (lo + st);
            u = (1.0 - beta) * u + beta * tanh_r(sigma * u);
            let yk = k * lpf(&mut s[2], u, gg);
            let _ = yk - lpf(&mut s[1], yk, gg);
            yk / k
        } else {
            let hi = x - lpf(&mut s[1], x, gg);
            let st = e * s[2] + l * s[0];
            let u = a * (hi + st);
            let mut yk = k * u;
            yk = (1.0 - beta) * yk + beta * tanh_r(sigma * yk);
            let h2 = yk - lpf(&mut s[2], yk, gg);
            lpf(&mut s[0], h2, gg);
            yk / k
        };
        y
    }

    /// s: z1..z4, stored feedbacks phi1..phi3.
    #[inline]
    pub(crate) fn tick_diode(&mut self, x: f32, out: u8) -> f32 {
        self.step(1.0);
        let [g, g1, g2, g3, g4, gamma, gg, kappa] = self.c;
        let h = 0.5 * g;
        let s = &mut self.s;
        let b1 = 1.0 / (1.0 + g - g * g2);
        let b2 = 1.0 / (1.0 + g - h * g3);
        let b3 = 1.0 / (1.0 + g - h * g4);
        let b4 = 1.0 / (1.0 + g);
        let c1 = 1.0 + g1 * g2;
        let c2 = 1.0 + g2 * g3;
        let c3 = 1.0 + g3 * g4;
        let fo = |b: f32, d: f32, p: f32, z: f32| (p * d + z) * b;
        let f3 = s[3] * b4;
        let f2 = fo(b3, h, s[6], s[2]);
        let f1 = fo(b2, h, s[5], s[1]);
        let sum = g4 * g3 * g2 * fo(b1, g, f1, s[0]) + g4 * g3 * fo(b2, h, f2, s[1]) + g4 * fo(b3, h, f3, s[2]) + s[3] * b4;
        s[6] = f3;
        s[5] = f2;
        s[4] = f1;
        let u = ((0.3 * kappa + 1.0) * x - kappa * sum) / (kappa * gamma + 1.0);
        let stage = |inp: f32, c: f32, p: f32, eps: f32, ofb: f32, mu: f32, z: &mut f32| {
            let i = inp * c + p + eps * ofb;
            let v = (mu * i - *z) * gg;
            let o = v + *z;
            *z = v + o;
            o
        };
        let fb1 = fo(b1, g, f1, s[0]);
        let o1 = stage(u, c1, f1, g2, fb1, 1.0, &mut s[0]);
        let fb2 = fo(b2, h, f2, s[1]);
        let o2 = stage(o1, c2, f2, g3, fb2, 0.5, &mut s[1]);
        let fb3 = fo(b3, h, f3, s[2]);
        let o3 = stage(o2, c3, f3, g4, fb3, 0.5, &mut s[2]);
        let o4 = stage(o3, 1.0, 0.0, 0.0, 0.0, 0.5, &mut s[3]);
        match out {
            0 => 0.125 * o1,
            1 => 0.3 * o2,
            2 => 0.6 * o3,
            _ => 1.2 * o4,
        }
    }

    /// s: i1, i2.
    #[inline]
    pub(crate) fn tick_fastsvf(&mut self, x: f32) -> f32 {
        self.step(1.0);
        let [a1, a2, a3, m0, m1, m2, ..] = self.c;
        let s = &mut self.s;
        let v3 = x - s[1];
        let v1 = a1 * s[0] + a2 * v3;
        let v2 = s[1] + a2 * s[0] + a3 * v3;
        s[0] = 2.0 * v1 - s[0];
        s[1] = 2.0 * v2 - s[1];
        m0 * x + m1 * v1 + m2 * v2
    }
}
