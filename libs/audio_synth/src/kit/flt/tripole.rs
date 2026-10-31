//! Tri-pole (FILTERS.md 5.14): three nonlinear OTA one-poles (each low or
//! high pass by mode) with a shaped global resonance loop, solved by three
//! global Newton steps per sample.

use super::approx::exp_r;
use super::{hz_clamped, FilterUnit, NC};
use std::f32::consts::PI;

/// [a0, a1, a2, b0, b1, b2, k].
pub(crate) fn make(n: f32, r: f32, fs: f32) -> [f32; NC] {
    let wc = 2.0 * PI * hz_clamped(n, fs) / fs;
    let e = exp_r(wc) - 1.0;
    let (b0, b1, b2) = (0.998 * e, 1.0012 * e, e);
    let k = -(1760f32.powf(r.clamp(0.0, 1.0)) + 1.0);
    [1.0 / (1.0 + b0), 1.0 / (1.0 + b1), 1.0 / (1.0 + b2), b0, b1, b2, k, 0.0]
}

#[inline]
fn ota(x: f32, beta: f32) -> f32 {
    x / (x * x + beta).sqrt()
}

/// The resonance shaper and its slope.
#[inline]
fn shaper(x: f32) -> f32 {
    let xx = 10.0 * x;
    if xx.abs() < 7.5 {
        0.99 * x
    } else {
        let lam = 9.03240196 * 1.0168177f32.ln();
        xx.signum() * (8.2 - exp_r(-lam * (xx + 0.222161).abs())) * 0.099
    }
}

#[inline]
fn shaper_slope(x: f32) -> f32 {
    let xx = 10.0 * x;
    if xx.abs() < 7.5 {
        0.99
    } else {
        let lam = 9.03240196 * 1.0168177f32.ln();
        exp_r(-lam * (xx + 0.222161).abs()) + lam / 10.0
    }
}

/// LP stage on a sigmoided input `tx` with state `z` (one Newton step).
#[inline]
fn lp_stage(tx: f32, z: f32, a: f32, b: f32, beta_out: f32) -> f32 {
    let mut e = a * (b * tx + z);
    let ty = ota(e, beta_out);
    let res = b * (tx - ty) + z - e;
    e -= res / (-b * (1.0 - ty * ty) - 1.0);
    e
}

/// HP stage on w = u - xp + z.
#[inline]
fn hp_stage(w: f32, a: f32, b: f32, beta_out: f32) -> f32 {
    let mut e = a * w;
    let ty = ota(e, beta_out);
    let res = -b * ty + w - e;
    e -= res / (-b * (1.0 - ty * ty) - 1.0);
    e
}

impl FilterUnit {
    /// s: z0, z1, z2, xp0, xp1, xp2, F, F_prev.
    pub(crate) fn tick_tripole(&mut self, x: f32, mode: u8, out: u8) -> f32 {
        let [a0, a1, a2, b0, b1, b2, k, _] = self.c;
        let x = 4.0 * x;
        let [z0, z1, z2, xp0, xp1, xp2, f_old, f_prev, ..] = self.s;
        let lp1 = mode <= 1;
        let lp2 = mode == 0 || mode == 2;
        let lp3 = mode <= 1;
        let w0 = x - xp0 + z0;
        let mut f = f_old;
        let (mut e0, mut e1, mut e2, mut ro) = (0.0, 0.0, 0.0, 0.0);
        for _ in 0..3 {
            let d0;
            if lp1 {
                let zz = z0 - f + f_prev;
                e0 = lp_stage(ota(x, 0.88), zz, a0, b0, 1.0);
                d0 = 2.0;
            } else {
                let tf = ota(f, 0.88);
                let mut e = a0 * (b0 * tf + w0);
                let ty = ota(e, 1.0);
                let res = b0 * (tf - ty) + w0 - e;
                e -= res / (-b0 * (1.0 - ty * ty) - 1.0);
                e0 = e;
                d0 = b0 * (1.0 - tf * tf);
            }
            let d1;
            if lp2 {
                let tx = ota(e0, 0.9);
                e1 = lp_stage(tx, z1, a1, b1, 0.97);
                d1 = b1 * (1.0 - tx * tx);
            } else {
                e1 = hp_stage(e0 - xp1 + z1, a1, b1, 0.97);
                d1 = 2.0;
            }
            ro = shaper(1.5 * k * e1) / 1.5;
            let dr = shaper_slope(k * e1);
            let d2;
            if lp3 {
                let tx = ota(ro, 0.95);
                e2 = lp_stage(tx, z2, a2, b2, 1.025);
                d2 = b2 * (1.0 - tx * tx);
            } else {
                e2 = hp_stage(ro - xp2 + z2, a2, b2, 1.025);
                d2 = 2.0;
            }
            f -= (f - e2) / (1.0 - k * dr * d0 * d1 * d2);
        }
        self.s[..8].copy_from_slice(&[e0, e1, e2, x, e0, ro, f, f_old]);
        self.step(1.0);
        0.25 * match out {
            0 => e0,
            1 => e1,
            _ => f,
        }
    }
}
