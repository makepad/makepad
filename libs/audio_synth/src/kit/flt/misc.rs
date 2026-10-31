//! Sample & hold and the two combs (FILTERS.md 5.5, 5.6).

use super::approx::sinc_kernel;
use super::{FilterUnit, NC};
use crate::kit::core::{note_hz, softclip};

pub(crate) const COMB_LEN: usize = 4096;
/// The extended line the Combulator asks for (very low comb frequencies):
/// 2^17 samples (+ guard), indices wrap modulo 2^17.
pub(crate) const COMB_LONG: usize = 1 << 17;
const GUARD: usize = 12;

/// The comb's delay line with 12 guard copies of its first samples.
#[derive(Clone, Debug)]
pub(crate) struct CombLine {
    buf: Vec<f32>,
    w: usize,
    len: usize,
}

impl Default for CombLine {
    fn default() -> Self {
        CombLine::new(COMB_LEN)
    }
}

impl CombLine {
    pub(crate) fn new(len: usize) -> CombLine {
        CombLine { buf: vec![0.0; len + GUARD], w: 0, len }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn clear(&mut self) {
        self.buf.iter_mut().for_each(|v| *v = 0.0);
        self.w = 0;
    }
}

/// [c, r].
pub(crate) fn make_snh(n: f32, r: f32, fs: f32) -> [f32; NC] {
    [note_hz(n) / fs, r, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
}

/// [delay, feedback, dry, wet]. `long` = the extended variant (EFFECTS
/// 5.16): delay clamp [12, 2^17 - 12] and the feedback clamped to [-2, 2]
/// with its sign (no Comb +/- sign rule); else [12, 4096 - 12] and
/// +-clamp01.
pub(crate) fn make_comb(ty: u8, sub: u8, n: f32, r: f32, fs: f32, correct: bool, long: bool) -> [f32; NC] {
    let mut d = fs / note_hz(n);
    if !correct {
        d -= 6.0;
    }
    let len = if long { COMB_LONG } else { COMB_LEN };
    let d = d.clamp(12.0, (len - 12) as f32);
    let fb = if long { r.clamp(-2.0, 2.0) } else { r.clamp(0.0, 1.0) * if ty == 25 { -1.0 } else { 1.0 } };
    let (dry, wet) = if sub == 1 { (0.0, 1.0) } else { (0.5, 0.5) };
    [d, fb, dry, wet, 0.0, 0.0, 0.0, 0.0]
}

impl FilterUnit {
    /// s: phase, held.
    #[inline]
    pub(crate) fn tick_snh(&mut self, x: f32) -> f32 {
        self.step(1.0);
        let [c, r, ..] = self.c;
        self.s[0] += c;
        if self.s[0] > 0.0 {
            self.s[1] = softclip(x - r * self.s[1]);
            self.s[0] -= 1.0;
        }
        self.s[1]
    }

    #[inline]
    pub(crate) fn tick_comb(&mut self, x: f32) -> f32 {
        self.step(1.0);
        let [d, fb, dry, wet, ..] = self.c;
        let Some(line) = self.comb.as_deref_mut() else { return x };
        let n = line.len;
        let d = d.min((n - 12) as f32);
        let e = (d * 256.0).round_ties_even() as i32;
        let dd = (e >> 8) as usize;
        let ph = 255 - (e & 255) as usize;
        let k = &sinc_kernel()[ph];
        let start = (line.w + 2 * n - dd - 6) % n;
        let mut read = 0.0f32;
        for i in 0..12 {
            read += line.buf[start + i] * k[i];
        }
        let v = softclip(x + fb * read);
        line.buf[line.w] = v;
        if line.w < GUARD {
            line.buf[n + line.w] = v;
        }
        line.w = (line.w + 1) % n;
        wet * read + dry * x
    }
}
