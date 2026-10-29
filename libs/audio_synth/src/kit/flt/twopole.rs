//! The OB-style 2-pole state-variable and 4-pole ladder (FILTERS.md 5.8,
//! 5.9), the latter also behind the cascade's 15 output mixes.

use super::{FilterUnit, NC};
use crate::kit::core::note_hz;
use std::f32::consts::PI;

fn g_of(n: f32, fs: f32) -> f32 {
    (PI * note_hz(n).min(22000.0) / fs).tan()
}

/// [g, R].
pub(crate) fn make_ob2(n: f32, r: f32, fs: f32) -> [f32; NC] {
    [g_of(n, fs), 1.0 - r, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
}

/// [g, K, rc, 1/rc].
pub(crate) fn make_ob4(n: f32, r: f32, fs: f32) -> [f32; NC] {
    let rc = (970.0 / 44000.0) * (44000.0 / fs).sqrt();
    [g_of(n, fs), 3.5 * r, rc, 1.0 / rc, 0.0, 0.0, 0.0, 0.0]
}

impl FilterUnit {
    /// s: s1, s2. mode 0 LP, 1 HP, 2 notch, 3 BP.
    #[inline]
    pub(crate) fn tick_ob2(&mut self, x: f32, mode: u8, push: bool) -> f32 {
        self.step(1.0);
        let [g, r, ..] = self.c;
        let (s1, s2) = (self.s[0], self.s[1]);
        let pv = 0.0876 * s1;
        let p = 1.0 + pv * (0.05 + pv * (0.185 + pv * (0.00920833 + pv * 0.0103592)));
        let t = p - if push { 1.035 } else { 1.0 };
        let v = (x - 2.0 * s1 * (r + t) - g * s1 - s2) / (1.0 + g * (2.0 * (r + t) + g));
        let y1 = v * g + s1;
        let ns1 = v * g + y1;
        let y2 = y1 * g + s2;
        let ns2 = y1 * g + y2;
        self.s[0] = ns1;
        self.s[1] = ns2;
        let out = match mode {
            0 => y2,
            1 => v,
            2 => 0.5 * y2 + 0.5 * v,
            _ => 0.5 * y1,
        };
        0.74 * out
    }

    /// s: s1..s4.
    #[inline]
    pub(crate) fn tick_ob4(&mut self, x: f32, w: &[f32; 5]) -> f32 {
        self.step(1.0);
        let [g, k, rc, rci, ..] = self.c;
        let s = &mut self.s;
        let p = g / (1.0 + g);
        let sum = (p * (p * (p * s[0] + s[1]) + s[2]) + s[3]) / (1.0 + g);
        let y0 = (x - k * sum) / (1.0 + k * p * p * p * p);
        let v = (y0 - s[0]) * p;
        let y1 = v + s[0];
        s[0] = ((y1 + v) * rc).atan() * rci;
        let tpt = |s: &mut f32, u: f32| {
            let v = (u - *s) * p;
            let o = v + *s;
            *s = o + v;
            o
        };
        let y2 = tpt(&mut s[1], y1);
        let y3 = tpt(&mut s[2], y2);
        let y4 = tpt(&mut s[3], y3);
        let mix = w[0] * y0 + w[1] * y1 + w[2] * y2 + w[3] * y3 + w[4] * y4;
        mix * (1.0 + 0.45 * k) * 0.6
    }
}
