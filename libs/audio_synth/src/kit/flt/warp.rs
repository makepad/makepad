//! Cutoff Warp and Resonance Warp (FILTERS.md 5.12, 5.13): 1-4 cascaded
//! biquads with a saturator in the feedback / on the states.

use super::approx::{cos_r, sin_r, tanh_r};
use super::{hz_clamped, FilterUnit, NC};
use crate::kit::core::softclip;
use crate::kit::shaper::ojd;
use std::f32::consts::PI;

const NT: [f32; 12] = [1.53273, 1.33407, 1.08197, 0.958219, 1.27374, 0.932342, 0.761765, 0.665462, 0.776856, 0.597575, 0.496207, 0.471714];

/// Response of a warp type: 0 LP, 1 HP, 2 notch, 3 BP, 4 allpass.
fn resp(ty: u8) -> u8 {
    match ty {
        16 | 28 => 0,
        17 | 29 => 1,
        18 | 30 => 2,
        19 | 31 => 3,
        _ => 4,
    }
}

/// (b0, b1, b2, a1, a2), r = clamped reso, nu = 2f/fs.
fn coefs(ty: u8, r: f32, w: f32) -> [f32; 5] {
    let q = 18.0 * r * r * r + 0.1;
    let (s, c) = (sin_r(w), cos_r(w));
    let al = s / (2.0 * q);
    let a = 1.0 / (1.0 + al);
    let a1 = -2.0 * c * a;
    let a2 = (1.0 - al) * a;
    let (b0, b1, b2) = match resp(ty) {
        0 => {
            let b1 = (1.0 - c) * a;
            (b1 / 2.0, b1, b1 / 2.0)
        }
        1 => {
            let b1 = -(1.0 + c) * a;
            (-b1 / 2.0, b1, -b1 / 2.0)
        }
        2 => (a, -2.0 * c * a, a),
        3 => {
            let b0 = (s / 2.0) * a;
            (b0, 0.0, -b0)
        }
        _ => (a2, a1, 1.0),
    };
    [b0, b1, b2, a1, a2]
}

/// [b0, b1, b2, a1, a2, makeup].
pub(crate) fn make_cwarp(ty: u8, sub: u8, n: f32, r: f32, fs: f32) -> [f32; NC] {
    let r = r.clamp(0.0, 1.0);
    let nu = 2.0 * hz_clamped(n, fs) / fs;
    let [b0, b1, b2, a1, a2] = coefs(ty, r, PI * nu);
    let rr = if sub < 8 {
        1.0
    } else {
        1.0 / r.max(if resp(ty) == 0 { 0.1 } else { 0.35 }).sqrt()
    };
    let nt = NT[(sub as usize).min(11)];
    let m = match resp(ty) {
        0 => rr * nt / nu.max(0.001).powf(0.1),
        1 => rr * nt / (1.0 - nu).max(0.001).powf(0.1),
        _ => 1.0,
    };
    [b0, b1, b2, a1, a2, m, 0.0, 0.0]
}

/// [b0, b1, b2, a1, a2].
pub(crate) fn make_rwarp(ty: u8, n: f32, r: f32, fs: f32) -> [f32; NC] {
    let r = r.clamp(0.0, 1.0);
    let w = 2.0 * PI * hz_clamped(n, fs) / fs;
    let [b0, b1, b2, a1, a2] = coefs(ty, r, w);
    [b0, b1, b2, a1, a2, 0.0, 0.0, 0.0]
}

#[inline]
fn sat(kind: u8, v: f32) -> f32 {
    match kind {
        0 => tanh_r(v),
        1 => softclip(v),
        _ => ojd(v),
    }
}

impl FilterUnit {
    /// s: (z1, z2) per stage.
    #[inline]
    pub(crate) fn tick_cwarp(&mut self, x: f32, stages: u8, kind: u8) -> f32 {
        let [b0, b1, b2, a1, a2, m, ..] = self.c;
        let mut u = x;
        for st in 0..stages as usize {
            let (z1, z2) = (self.s[2 * st], self.s[2 * st + 1]);
            let o = z1 + b0 * u;
            let nf = sat(kind, o);
            self.s[2 * st] = z2 + b1 * u - a1 * nf;
            self.s[2 * st + 1] = b2 * u - a2 * nf;
            u = o * m;
        }
        self.step(1.0);
        u
    }

    /// s: (z1, z2) per stage.
    #[inline]
    pub(crate) fn tick_rwarp(&mut self, x: f32, stages: u8, kind: u8) -> f32 {
        let [b0, b1, b2, a1, a2, ..] = self.c;
        let mut u = x;
        for st in 0..stages as usize {
            let (z1, z2) = (self.s[2 * st], self.s[2 * st + 1]);
            let o = z1 + b0 * u;
            self.s[2 * st] = sat(kind, z2 + b1 * u - a1 * o);
            self.s[2 * st + 1] = sat(kind, b2 * u - a2 * o);
            u = o;
        }
        self.step(1.0);
        u
    }
}
