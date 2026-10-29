//! The 12/24 dB families (FILTERS.md 5.1-5.3): the double-sampled SVF of
//! the Standard subtypes, the RBJ biquads of the Driven (coupled form) and
//! Clean (normalised lattice) subtypes, notch and allpass.

use super::approx::omega_sc;
use super::{FilterUnit, Resp, NC};
use crate::kit::core::note_hz;
use std::f32::consts::PI;

/// Standard subtype coefficients: [F, Q, D, G].
pub(crate) fn make_svf(ty: u8, n: f32, r: f32, fs: f32) -> [f32; NC] {
    let f = note_hz(n);
    let r = r.clamp(0.0, 1.0);
    let ff = 2.0 * (PI * (f / (2.0 * fs)).min(0.11)).sin();
    let rho = r.sqrt();
    let o = if matches!(ty, 2 | 5) { 0.1 } else { 0.15 };
    let q = (2.0 - rho * (2.0 + o) + ff * ff * o * 0.9).min(2.0).min(2.0 - 1.52 * ff);
    [ff, q, 0.1 * rho * ff, 1.0 - 0.65 * rho, 0.0, 0.0, 0.0, 0.0]
}

/// The biquad (b0, b1, b2, a1, a2) normalised by a0, its clip gain, and
/// whether it is realised in coupled form (Driven) or as a lattice.
pub(crate) fn recipe(ty: u8, sub: u8, n: f32, r: f32, fs: f32) -> ([f32; 5], f32, bool) {
    let n = n.clamp(-55.0, 75.0);
    let (s, c) = omega_sc(n, fs);
    let c2 = |r: f32| (1.0 - (1.0 - r) * (1.0 - r)).clamp(0.0, 1.0);
    match ty {
        7 | 24 | 26 => {
            let m = if ty != 26 && sub == 1 { 1.0 - 0.99 * c2(r) } else { 2.5 - 2.49 * c2(r) };
            let al = s * m;
            let (a0, a1, a2) = (1.0 + al, -2.0 * c, 1.0 - al);
            let b = if ty == 26 { [1.0 - al, -2.0 * c, 1.0 + al] } else { [1.0, -2.0 * c, 1.0] };
            (norm(a0, a1, a2, b), 0.005, false)
        }
        _ => {
            let driven = sub == 1;
            let four = matches!(ty, 2 | 5 | 23);
            let resp = super::resp_of(ty);
            let rp = r * (1.0 - ((n - 58.0) * 0.05).max(0.0)).max(0.0);
            let map = match (four, driven) {
                (false, true) => 1.0 - 1.05 * (1.0 - (1.0 - rp) * (1.0 - rp)).clamp(0.001, 1.0),
                (false, false) => 2.5 - 2.45 * c2(r),
                (true, true) => 1.0 - 1.05 * rp.clamp(0.001, 1.0),
                (true, false) => 2.5 - 2.3 * r.clamp(0.0, 1.0),
            };
            let mut al = s * map;
            if resp != Resp::Lp || driven {
                al = al.min(s.abs() - 0.0001);
            }
            let mut g = if driven { 1.0 - 0.5 * r * r } else { 1.0 - 0.25 * r * r };
            if driven && resp == Resp::Bp {
                g *= 2.0;
            }
            let b = match resp {
                Resp::Lp => [(1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5],
                Resp::Hp => [(1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5],
                _ => {
                    let b0 = (0.5 / map) * al;
                    [b0, 0.0, -b0]
                }
            };
            let gamma = if driven { (1.0 / 64.0) * 10f32.powf(0.0275 * n) } else { 1.0 / 1024.0 };
            (norm(1.0 + al, -2.0 * c, 1.0 - al, [b[0] * g, b[1] * g, b[2] * g]), gamma, driven)
        }
    }
}

fn norm(a0: f32, a1: f32, a2: f32, b: [f32; 3]) -> [f32; 5] {
    let ia = 1.0 / a0;
    [b[0] * ia, b[1] * ia, b[2] * ia, a1 * ia, a2 * ia]
}

/// Driven: [ar, ai, c1, c2, d, gamma]; Clean / notch / allpass:
/// [k1, k2, q1, q2, v1, v2, v3, gamma].
pub(crate) fn make(ty: u8, sub: u8, n: f32, r: f32, fs: f32) -> [f32; NC] {
    let ([b0, b1, b2, a1, a2], gamma, driven) = recipe(ty, sub, n, r, fs);
    if driven {
        let ar = -a1 / 2.0;
        let ai = (0.5 * (4.0 * a2 - a1 * a1).max(0.0).sqrt()).max(8.0 * 1.19e-7);
        let bb1 = b1 - a1 * b0;
        let bb2 = b2 - a2 * b0;
        [ar, ai, bb1, (bb1 * ar + bb2) / ai, b0, gamma, 0.0, 0.0]
    } else {
        let k1 = a1 / (1.0 + a2);
        let k2 = a2;
        let q1 = (1.0 - k1 * k1).abs().sqrt();
        let q2 = (1.0 - k2 * k2).abs().sqrt();
        let v3 = b2;
        let v2 = (b1 - a1 * v3) / q2;
        let v1 = (b0 - k1 * v2 * q2 - k2 * v3) / (q1 * q2);
        [k1, k2, q1, q2, v1, v2, v3, gamma]
    }
}

impl FilterUnit {
    /// s: b1, l1, b2, l2, k.
    #[inline]
    pub(crate) fn tick_svf(&mut self, x: f32, r: Resp, four: bool) -> f32 {
        self.step(1.0);
        let [f, q, d, g, ..] = self.c;
        let k = self.s[4];
        #[inline]
        fn stage(b: &mut f32, l: &mut f32, u: f32, f: f32, q: f32, k: f32) -> (f32, f32, f32, f32) {
            let l1 = *l + f * *b;
            let h1 = u - l1 - q * *b;
            let b1 = *b + f * h1;
            let l2 = l1 + f * b1;
            let h2 = u - l2 - q * b1;
            let b2 = b1 + f * h2;
            *b = b2 * k;
            *l = l2 * k;
            (b1, l2, h2, b2)
        }
        let pick = |(_, l2, h2, b2): (f32, f32, f32, f32)| match r {
            Resp::Lp => l2,
            Resp::Hp => h2,
            _ => b2,
        };
        let (mut b, mut l) = (self.s[0], self.s[1]);
        let o1 = stage(&mut b, &mut l, x, f, q, k);
        self.s[0] = b;
        self.s[1] = l;
        if !four {
            self.s[4] = (1.0 - d * o1.0 * o1.0).max(0.1);
            return g * pick(o1);
        }
        let (mut b, mut l) = (self.s[2], self.s[3]);
        let o2 = stage(&mut b, &mut l, pick(o1), f, q, k);
        self.s[2] = b;
        self.s[3] = l;
        self.s[4] = (1.0 - d * o2.3 * o2.3).max(0.1);
        g * pick(o2)
    }

    /// s: s1, s2, s3, s4, k.
    #[inline]
    pub(crate) fn tick_cfc(&mut self, x: f32, two: bool) -> f32 {
        self.step(1.0);
        let [ar, ai, c1, c2, d, gamma, ..] = self.c;
        let k = self.s[4];
        let sec = |s1: &mut f32, s2: &mut f32, x: f32| {
            let y = c1 * *s1 + c2 * *s2 + d * x;
            let n1 = ar * *s1 - ai * *s2 + x;
            let n2 = ai * *s1 + ar * *s2;
            *s1 = n1 * k;
            *s2 = n2 * k;
            y
        };
        let (mut s1, mut s2, mut s3, mut s4) = (self.s[0], self.s[1], self.s[2], self.s[3]);
        let mut y = sec(&mut s1, &mut s2, x);
        if two {
            y = sec(&mut s3, &mut s4, y);
        }
        self.s[..4].copy_from_slice(&[s1, s2, s3, s4]);
        self.s[4] = (1.0 - gamma * y * y).max(0.1);
        y
    }

    /// s: r0, r1, r2, r3, k.
    #[inline]
    pub(crate) fn tick_lattice(&mut self, x: f32, two: bool) -> f32 {
        self.step(1.0);
        let [k1, k2, q1, q2, v1, v2, v3, gamma] = self.c;
        let k = self.s[4];
        let sec = |r0: &mut f32, r1: &mut f32, x: f32| {
            let f2 = q2 * x - k2 * *r1;
            let g2 = k2 * x + q2 * *r1;
            let f1 = q1 * f2 - k1 * *r0;
            let g1 = k1 * f2 + q1 * *r0;
            let y = v3 * g2 + v2 * g1 + v1 * f1;
            *r0 = f1 * k;
            *r1 = g1 * k;
            y
        };
        let (mut r0, mut r1, mut r2, mut r3) = (self.s[0], self.s[1], self.s[2], self.s[3]);
        let mut y = sec(&mut r0, &mut r1, x);
        if two {
            y = sec(&mut r2, &mut r3, y);
        }
        self.s[..4].copy_from_slice(&[r0, r1, r2, r3]);
        self.s[4] = (1.0 - gamma * y * y).max(0.1);
        y
    }
}
