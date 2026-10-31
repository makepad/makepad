//! The encoder's hybrid filterbank (ISO/IEC 11172-3 Annex C.1.3 and C.1.5.3):
//! the 32-band polyphase analysis filter, the frequency inversion of the odd
//! subbands, the 18/6-line MDCT per subband and the alias-reduction
//! butterflies. Each stage is the exact inverse of the decoder stage in
//! `makepad_audio_decode::mp3`, which is what the round-trip tests check.
//!
//! The analysis window C is the synthesis window D of Table 3-B.3 divided by
//! 32 (the standard tabulates both; they are the same curve), so it is taken
//! from the decoder's table instead of being transcribed twice.

use makepad_audio_decode::mp3::tables::synth_window;
use makepad_audio_decode::mp3::tables_lsf::alias_coefficients;

pub const LINES: usize = 576;
pub const SLOTS: usize = 18;
pub const SUBBANDS: usize = 32;

/// Block types of a granule (`block_type` in the side info).
pub mod block {
    pub const NORMAL: u8 = 0;
    pub const START: u8 = 1;
    pub const SHORT: u8 = 2;
    pub const STOP: u8 = 3;
}

pub struct Analysis {
    /// Analysis window C, reversed so a slot's 512 input samples (oldest
    /// first) multiply it element by element.
    c_rev: [f32; 512],
    /// Polyphase matrixing, `m[sb][k] = cos((2sb+1)(k-16)pi/64)`.
    m: Box<[[f32; 64]; SUBBANDS]>,
    /// Long windows (block types 0, 1, 3) folded into the 36-point MDCT basis:
    /// `long[bt][k][n] = window[bt][n] * cos(pi/72 (2n+1+18)(2k+1))`.
    long: Box<[[[f32; 36]; SLOTS]; 4]>,
    /// Short window folded into the 12-point basis.
    short: [[f32; 12]; 6],
    cs: [f32; 8],
    ca: [f32; 8],
}

impl Default for Analysis {
    fn default() -> Self {
        Self::new()
    }
}

impl Analysis {
    pub fn new() -> Self {
        use std::f64::consts::PI;
        let d = synth_window();
        let mut c_rev = [0.0f32; 512];
        for i in 0..512 {
            c_rev[511 - i] = d[i] / 32.0;
        }
        let mut m = Box::new([[0.0f32; 64]; SUBBANDS]);
        for (sb, row) in m.iter_mut().enumerate() {
            for (k, v) in row.iter_mut().enumerate() {
                *v = ((2 * sb + 1) as f64 * (k as f64 - 16.0) * PI / 64.0).cos() as f32;
            }
        }
        // The decoder's IMDCT carries no normalisation, so the forward
        // transform divides by the TDAC gain n/4 (9 for 36 points, 3 for 12).
        let mut window = [[0.0f64; 36]; 4];
        for n in 0..36 {
            let nf = n as f64;
            window[0][n] = (PI / 36.0 * (nf + 0.5)).sin();
            window[1][n] = match n {
                0..=17 => (PI / 36.0 * (nf + 0.5)).sin(),
                18..=23 => 1.0,
                24..=29 => (PI / 12.0 * (nf - 18.0 + 0.5)).sin(),
                _ => 0.0,
            };
            window[3][n] = match n {
                0..=5 => 0.0,
                6..=11 => (PI / 12.0 * (nf - 6.0 + 0.5)).sin(),
                12..=17 => 1.0,
                _ => (PI / 36.0 * (nf + 0.5)).sin(),
            };
        }
        let mut long = Box::new([[[0.0f32; 36]; SLOTS]; 4]);
        for bt in [0usize, 1, 3] {
            for k in 0..SLOTS {
                for n in 0..36 {
                    let basis = (PI / 72.0 * (2 * n + 1 + 18) as f64 * (2 * k + 1) as f64).cos();
                    long[bt][k][n] = (window[bt][n] * basis / 9.0) as f32;
                }
            }
        }
        let mut short = [[0.0f32; 12]; 6];
        for (k, row) in short.iter_mut().enumerate() {
            for (n, v) in row.iter_mut().enumerate() {
                let w = (PI / 12.0 * (n as f64 + 0.5)).sin();
                let basis = (PI / 24.0 * (2 * n + 1 + 6) as f64 * (2 * k + 1) as f64).cos();
                *v = (w * basis / 3.0) as f32;
            }
        }
        let (cs, ca) = alias_coefficients();
        Self { c_rev, m, long, short, cs, ca }
    }

    /// The 32 subband samples of polyphase slot `slot`: input samples
    /// `32*slot - 480 ..= 32*slot + 31` of `pcm` (zero outside it), newest
    /// last. Odd subbands come out frequency-inverted on odd slots, which is
    /// what the MDCT stage expects.
    pub fn slot(&self, pcm: &[f32], slot: i64, out: &mut [f32; SUBBANDS]) {
        let newest = slot * 32 + 31;
        let oldest = newest - 511;
        let mut y = [0.0f32; 64];
        if oldest >= 0 && (newest as usize) < pcm.len() {
            let x = &pcm[oldest as usize..=newest as usize];
            // z[i] = C[i] X[i] with X[i] = x[511 - i]; y[i] = sum_j z[i + 64 j].
            for j in 0..8 {
                let base = 511 - 64 * j;
                for i in 0..64 {
                    y[i] += self.c_rev[base - i] * x[base - i];
                }
            }
        } else {
            for (idx, slot_c) in self.c_rev.iter().enumerate() {
                let at = oldest + idx as i64;
                if at < 0 || at as usize >= pcm.len() {
                    continue;
                }
                let i = (511 - idx) % 64;
                y[i] += slot_c * pcm[at as usize];
            }
        }
        for (sb, row) in self.m.iter().enumerate() {
            let mut acc = 0.0f32;
            for k in 0..64 {
                acc += row[k] * y[k];
            }
            out[sb] = acc;
        }
        if slot & 1 == 1 {
            for sb in (1..SUBBANDS).step_by(2) {
                out[sb] = -out[sb];
            }
        }
    }

    /// One granule's 18 slots of subband samples (`[slot][subband]`).
    pub fn granule_subbands(&self, pcm: &[f32], granule: i64, out: &mut [[f32; SUBBANDS]; SLOTS]) {
        for (s, row) in out.iter_mut().enumerate() {
            self.slot(pcm, granule * SLOTS as i64 + s as i64, row);
        }
    }

    /// MDCT of one granule from its own and the previous granule's subband
    /// samples. Output is in subband order (`sb*18 + k`); a short block's
    /// three windows are interleaved as `sb*18 + 3k + w`, the layout the
    /// decoder's IMDCT reads after its reorder.
    pub fn mdct(
        &self,
        prev: &[[f32; SUBBANDS]; SLOTS],
        cur: &[[f32; SUBBANDS]; SLOTS],
        block_type: u8,
        xr: &mut [f32; LINES],
    ) {
        let mut z = [0.0f32; 36];
        for sb in 0..SUBBANDS {
            for s in 0..SLOTS {
                z[s] = prev[s][sb];
                z[s + SLOTS] = cur[s][sb];
            }
            let out = &mut xr[sb * SLOTS..sb * SLOTS + SLOTS];
            if block_type == block::SHORT {
                for w in 0..3 {
                    let seg = &z[6 + 6 * w..18 + 6 * w];
                    for (k, row) in self.short.iter().enumerate() {
                        let mut acc = 0.0f32;
                        for n in 0..12 {
                            acc += row[n] * seg[n];
                        }
                        out[3 * k + w] = acc;
                    }
                }
            } else {
                let basis = &self.long[block_type as usize];
                for (k, row) in basis.iter().enumerate() {
                    let mut acc = 0.0f32;
                    for n in 0..36 {
                        acc += row[n] * z[n];
                    }
                    out[k] = acc;
                }
            }
        }
        if block_type != block::SHORT {
            // The inverse of the decoder's butterfly at every subband boundary.
            for sb in 0..SUBBANDS - 1 {
                let base = sb * SLOTS;
                for i in 0..8 {
                    let lo = base + 17 - i;
                    let hi = base + 18 + i;
                    let (a, b) = (xr[lo], xr[hi]);
                    xr[lo] = a * self.cs[i] + b * self.ca[i];
                    xr[hi] = b * self.cs[i] - a * self.ca[i];
                }
            }
        }
    }
}

/// Short-block lines from the filterbank's interleaved layout into the
/// bitstream's order: by scalefactor band, then window, then frequency.
pub fn short_to_bitstream(sfb_short: &[u16; 14], xr: &[f32; LINES], out: &mut [f32; LINES]) {
    let mut dst = 0usize;
    for sfb in 0..13 {
        let start = sfb_short[sfb] as usize;
        let width = (sfb_short[sfb + 1] - sfb_short[sfb]) as usize;
        for w in 0..3 {
            for f in 0..width {
                out[dst] = xr[(start + f) * 3 + w];
                dst += 1;
            }
        }
    }
}
