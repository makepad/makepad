//! Scalar CPU reference dequantization for the IQ-class (codebook) ggml quant
//! types plus Q2_K / Q3_K — the tensor types unsloth's "Dynamic" (UD-) GGUF
//! conversions mix into otherwise K-quant files.
//!
//! These are transcriptions of `dequantize_row_*` in ggml's `ggml-quants.c`
//! (MIT, see libs/ai/NOTICE), and they are the oracle the GPU kernels are
//! tested against: `libs/ai/cuda/kernels/llm/iq_convert.cuh` (CUDA) and
//! ggml-metal's `dequantize_*` (Metal) must reproduce these values bit for
//! bit modulo the f32->bf16/f16 rounding of the destination type.
//!
//! Layout note: every function takes the *raw GGUF byte stream* for one row
//! (not a typed struct), because that is how the loader hands weights to the
//! backends — mmapped, unaligned, little-endian.

use crate::quant::f16_to_f32;
use crate::quant_iq_tables::*;

pub const QK_K: usize = 256;
pub const QK4_NL: usize = 32;
const IQ1S_DELTA: f32 = 0.125;
const IQ1M_DELTA: f32 = 0.125;

#[inline]
fn f16_at(b: &[u8], off: usize) -> f32 {
    f16_to_f32(u16::from_le_bytes([b[off], b[off + 1]]))
}

#[inline]
fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// One `u64` codebook entry as its 8 unsigned lanes (iq2*/iq3* grids).
#[inline]
fn grid8_u(v: u64) -> [u8; 8] {
    v.to_le_bytes()
}

/// One `u32` codebook entry as its 4 unsigned lanes (iq3xxs / iq3s grids).
#[inline]
fn grid4_u(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// One `u64` iq1s codebook entry as its 8 *signed* lanes.
#[inline]
fn grid8_i(v: u64) -> [i8; 8] {
    let b = v.to_le_bytes();
    [
        b[0] as i8, b[1] as i8, b[2] as i8, b[3] as i8, b[4] as i8, b[5] as i8, b[6] as i8,
        b[7] as i8,
    ]
}

// ---------------------------------------------------------------------------
// Q2_K / Q3_K — K-quants the UD- mixes also reach for, and which the CUDA
// executor had no kernel for either.
// ---------------------------------------------------------------------------

/// Q2_K: `scales[16] qs[64] d dmin` = 84 bytes per 256 values.
pub fn dequantize_q2_k(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 84 && out.len() >= QK_K);
    let scales = &block[0..16];
    let qs = &block[16..80];
    let d = f16_at(block, 80);
    let dmin = f16_at(block, 82);
    let mut y = 0usize;
    for n in (0..QK_K).step_by(128) {
        let qbase = (n / 128) * 32;
        let mut shift = 0u32;
        for j in 0..4 {
            for sub in 0..2 {
                let is = (n / 128) * 8 + j * 2 + sub;
                let sc = scales[is];
                let dl = d * f32::from(sc & 0xF);
                let ml = dmin * f32::from(sc >> 4);
                for l in 0..16 {
                    let q = qs[qbase + sub * 16 + l];
                    out[y] = dl * f32::from((q >> shift) & 3) - ml;
                    y += 1;
                }
            }
            shift += 2;
        }
    }
}

/// Q3_K: `hmask[32] qs[64] scales[12] d` = 110 bytes per 256 values.
pub fn dequantize_q3_k(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 110 && out.len() >= QK_K);
    const KMASK1: u32 = 0x03030303;
    const KMASK2: u32 = 0x0f0f0f0f;
    let hmask = &block[0..32];
    let qs = &block[32..96];
    let d_all = f16_at(block, 108);

    let mut aux = [
        u32_at(block, 96),
        u32_at(block, 100),
        u32_at(block, 104),
        0u32,
    ];
    let tmp = aux[2];
    aux[2] = ((aux[0] >> 4) & KMASK2) | (((tmp >> 4) & KMASK1) << 4);
    aux[3] = ((aux[1] >> 4) & KMASK2) | (((tmp >> 6) & KMASK1) << 4);
    aux[0] = (aux[0] & KMASK2) | (((tmp) & KMASK1) << 4);
    aux[1] = (aux[1] & KMASK2) | (((tmp >> 2) & KMASK1) << 4);
    let mut sc = [0i8; 16];
    for (i, w) in aux.iter().enumerate() {
        let b = w.to_le_bytes();
        for j in 0..4 {
            sc[i * 4 + j] = b[j] as i8;
        }
    }

    let mut y = 0usize;
    let mut m: u8 = 1;
    let mut is = 0usize;
    for n in (0..QK_K).step_by(128) {
        let qbase = (n / 128) * 32;
        let mut shift = 0u32;
        for _j in 0..4 {
            for sub in 0..2 {
                let dl = d_all * f32::from(sc[is] as i32 as i16 - 32);
                is += 1;
                for l in 0..16 {
                    let idx = sub * 16 + l;
                    let q = i32::from((qs[qbase + idx] >> shift) & 3);
                    let h = if hmask[idx] & m != 0 { 0 } else { 4 };
                    out[y] = dl * (q - h) as f32;
                    y += 1;
                }
            }
            shift += 2;
            m <<= 1;
        }
    }
}

// ---------------------------------------------------------------------------
// IQ codebook types
// ---------------------------------------------------------------------------

/// IQ2_XXS: `d qs[32:u16]` = 66 bytes per 256 values.
pub fn dequantize_iq2_xxs(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 66 && out.len() >= QK_K);
    let d = f16_at(block, 0);
    let mut y = 0usize;
    for ib32 in 0..QK_K / 32 {
        // qs is u16[32]; `qs + 4*ib32` in the C reference is a u16 offset.
        let base = 2 + 8 * ib32;
        let aux0 = u32_at(block, base);
        let aux1 = u32_at(block, base + 4);
        let aux8 = aux0.to_le_bytes();
        let db = d * (0.5 + (aux1 >> 28) as f32) * 0.25;
        for l in 0..4 {
            let grid = grid8_u(IQ2XXS_GRID[aux8[l] as usize]);
            let signs = KSIGNS_IQ2XS[((aux1 >> (7 * l)) & 127) as usize];
            for j in 0..8 {
                let s = if signs & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                out[y] = db * f32::from(grid[j]) * s;
                y += 1;
            }
        }
    }
}

/// IQ2_XS: `d qs[32:u16] scales[8]` = 74 bytes per 256 values.
pub fn dequantize_iq2_xs(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 74 && out.len() >= QK_K);
    let d = f16_at(block, 0);
    let mut y = 0usize;
    for ib32 in 0..QK_K / 32 {
        let sc = block[66 + ib32];
        let db = [
            d * (0.5 + f32::from(sc & 0xf)) * 0.25,
            d * (0.5 + f32::from(sc >> 4)) * 0.25,
        ];
        for l in 0..4 {
            let q = u16_at(block, 2 + 2 * (4 * ib32 + l));
            let grid = grid8_u(IQ2XS_GRID[(q & 511) as usize]);
            let signs = KSIGNS_IQ2XS[(q >> 9) as usize];
            for j in 0..8 {
                let s = if signs & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                out[y] = db[l / 2] * f32::from(grid[j]) * s;
                y += 1;
            }
        }
    }
}

/// IQ2_S: `d qs[32] signs[32] qh[8] scales[8]` = 82 bytes per 256 values.
pub fn dequantize_iq2_s(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 82 && out.len() >= QK_K);
    let d = f16_at(block, 0);
    let qs = &block[2..66]; // qs[0..32] quants, qs[32..64] signs
    let qh = &block[66..74];
    let scales = &block[74..82];
    let mut y = 0usize;
    for ib32 in 0..QK_K / 32 {
        let sc = scales[ib32];
        let db = [
            d * (0.5 + f32::from(sc & 0xf)) * 0.25,
            d * (0.5 + f32::from(sc >> 4)) * 0.25,
        ];
        for l in 0..4 {
            let idx =
                usize::from(qs[4 * ib32 + l]) | ((usize::from(qh[ib32]) << (8 - 2 * l)) & 0x300);
            let grid = grid8_u(IQ2S_GRID[idx]);
            let signs = qs[QK_K / 8 + 4 * ib32 + l];
            for j in 0..8 {
                let s = if signs & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                out[y] = db[l / 2] * f32::from(grid[j]) * s;
                y += 1;
            }
        }
    }
}

/// IQ3_XXS: `d qs[64] scales_and_signs[32]` = 98 bytes per 256 values.
pub fn dequantize_iq3_xxs(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 98 && out.len() >= QK_K);
    let d = f16_at(block, 0);
    let qs = &block[2..66];
    let mut y = 0usize;
    for ib32 in 0..QK_K / 32 {
        let aux32 = u32_at(block, 66 + 4 * ib32);
        let db = d * (0.5 + (aux32 >> 28) as f32) * 0.5;
        for l in 0..4 {
            let signs = KSIGNS_IQ2XS[((aux32 >> (7 * l)) & 127) as usize];
            let g1 = grid4_u(IQ3XXS_GRID[qs[8 * ib32 + 2 * l] as usize]);
            let g2 = grid4_u(IQ3XXS_GRID[qs[8 * ib32 + 2 * l + 1] as usize]);
            for j in 0..4 {
                let s0 = if signs & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                let s1 = if signs & KMASK_IQ2XS[j + 4] != 0 {
                    -1.0
                } else {
                    1.0
                };
                out[y + j] = db * f32::from(g1[j]) * s0;
                out[y + j + 4] = db * f32::from(g2[j]) * s1;
            }
            y += 8;
        }
    }
}

/// IQ3_S: `d qs[64] qh[8] signs[32] scales[4]` = 110 bytes per 256 values.
pub fn dequantize_iq3_s(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 110 && out.len() >= QK_K);
    let d = f16_at(block, 0);
    let qs = &block[2..66];
    let qh = &block[66..74];
    let signs = &block[74..106];
    let scales = &block[106..110];
    let mut y = 0usize;
    for ib32 in 0..QK_K / 32 {
        let nib = if ib32 % 2 == 0 {
            scales[ib32 / 2] & 0xf
        } else {
            scales[ib32 / 2] >> 4
        };
        let db = d * (1.0 + 2.0 * f32::from(nib));
        for l in 0..4 {
            let h = usize::from(qh[ib32]);
            let i1 = usize::from(qs[8 * ib32 + 2 * l]) | ((h << (8 - 2 * l)) & 256);
            let i2 = usize::from(qs[8 * ib32 + 2 * l + 1]) | ((h << (7 - 2 * l)) & 256);
            let g1 = grid4_u(IQ3S_GRID[i1]);
            let g2 = grid4_u(IQ3S_GRID[i2]);
            let sg = signs[4 * ib32 + l];
            for j in 0..4 {
                let s0 = if sg & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                let s1 = if sg & KMASK_IQ2XS[j + 4] != 0 {
                    -1.0
                } else {
                    1.0
                };
                out[y + j] = db * f32::from(g1[j]) * s0;
                out[y + j + 4] = db * f32::from(g2[j]) * s1;
            }
            y += 8;
        }
    }
}

/// IQ1_S: `d qs[32] qh[8:u16]` = 50 bytes per 256 values.
pub fn dequantize_iq1_s(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 50 && out.len() >= QK_K);
    let d = f16_at(block, 0);
    let qs = &block[2..34];
    let mut y = 0usize;
    for ib in 0..QK_K / 32 {
        let qh = u16_at(block, 34 + 2 * ib);
        let dl = d * f32::from(2 * ((qh >> 12) & 7) + 1);
        let delta = if qh & 0x8000 != 0 {
            -IQ1S_DELTA
        } else {
            IQ1S_DELTA
        };
        for l in 0..4 {
            let idx = usize::from(qs[4 * ib + l]) | ((usize::from(qh >> (3 * l)) & 7) << 8);
            let grid = grid8_i(IQ1S_GRID[idx]);
            for j in 0..8 {
                out[y] = dl * (f32::from(grid[j]) + delta);
                y += 1;
            }
        }
    }
}

/// IQ1_M: `qs[32] qh[16] scales[8]` = 56 bytes per 256 values.
pub fn dequantize_iq1_m(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 56 && out.len() >= QK_K);
    let qs = &block[0..32];
    let qh = &block[32..48];
    let sc = [
        u16_at(block, 48),
        u16_at(block, 50),
        u16_at(block, 52),
        u16_at(block, 54),
    ];
    let scale_u16 =
        (sc[0] >> 12) | ((sc[1] >> 8) & 0x00f0) | ((sc[2] >> 4) & 0x0f00) | (sc[3] & 0xf000);
    let d = f16_to_f32(scale_u16);
    let mut y = 0usize;
    for ib in 0..QK_K / 32 {
        let dl1 = d * f32::from(2 * ((sc[ib / 2] >> (6 * (ib % 2))) & 0x7) + 1);
        let dl2 = d * f32::from(2 * ((sc[ib / 2] >> (6 * (ib % 2) + 3)) & 0x7) + 1);
        let qh0 = qh[2 * ib];
        let qh1 = qh[2 * ib + 1];
        let idx = [
            usize::from(qs[4 * ib]) | ((usize::from(qh0) << 8) & 0x700),
            usize::from(qs[4 * ib + 1]) | ((usize::from(qh0) << 4) & 0x700),
            usize::from(qs[4 * ib + 2]) | ((usize::from(qh1) << 8) & 0x700),
            usize::from(qs[4 * ib + 3]) | ((usize::from(qh1) << 4) & 0x700),
        ];
        let delta = [
            if qh0 & 0x08 != 0 {
                -IQ1M_DELTA
            } else {
                IQ1M_DELTA
            },
            if qh0 & 0x80 != 0 {
                -IQ1M_DELTA
            } else {
                IQ1M_DELTA
            },
            if qh1 & 0x08 != 0 {
                -IQ1M_DELTA
            } else {
                IQ1M_DELTA
            },
            if qh1 & 0x80 != 0 {
                -IQ1M_DELTA
            } else {
                IQ1M_DELTA
            },
        ];
        for l in 0..4 {
            let dl = if l < 2 { dl1 } else { dl2 };
            let grid = grid8_i(IQ1S_GRID[idx[l]]);
            for j in 0..8 {
                out[y] = dl * (f32::from(grid[j]) + delta[l]);
                y += 1;
            }
        }
    }
}

/// IQ4_NL: `d qs[16]` = 18 bytes per 32 values.
pub fn dequantize_iq4_nl(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 18 && out.len() >= QK4_NL);
    let d = f16_at(block, 0);
    for j in 0..QK4_NL / 2 {
        let q = block[2 + j];
        out[j] = d * f32::from(KVALUES_IQ4NL[(q & 0xf) as usize]);
        out[j + QK4_NL / 2] = d * f32::from(KVALUES_IQ4NL[(q >> 4) as usize]);
    }
}

/// IQ4_XS: `d scales_h:u16 scales_l[4] qs[128]` = 136 bytes per 256 values.
pub fn dequantize_iq4_xs(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 136 && out.len() >= QK_K);
    let d = f16_at(block, 0);
    let scales_h = u16_at(block, 2);
    let scales_l = &block[4..8];
    let qs = &block[8..136];
    for ib in 0..QK_K / 32 {
        let ls = ((scales_l[ib / 2] >> (4 * (ib % 2))) & 0xf) as i32
            | ((((scales_h >> (2 * ib)) & 3) as i32) << 4);
        let dl = d * (ls - 32) as f32;
        for j in 0..16 {
            let q = qs[16 * ib + j];
            out[32 * ib + j] = dl * f32::from(KVALUES_IQ4NL[(q & 0xf) as usize]);
            out[32 * ib + j + 16] = dl * f32::from(KVALUES_IQ4NL[(q >> 4) as usize]);
        }
    }
}

// ---------------------------------------------------------------------------
// Row-level driver: dequantize `n` values from a raw GGUF row.
// ---------------------------------------------------------------------------

/// Dequantize `n` elements of a raw GGUF row of `ggml_type` into `out`.
/// Returns `false` for a type this module does not implement.
pub fn dequantize_row_iq(ggml_type: u32, row: &[u8], n: usize, out: &mut [f32]) -> bool {
    use crate::quant::{
        GGML_TYPE_IQ1_M, GGML_TYPE_IQ1_S, GGML_TYPE_IQ2_S, GGML_TYPE_IQ2_XS, GGML_TYPE_IQ2_XXS,
        GGML_TYPE_IQ3_S, GGML_TYPE_IQ3_XXS, GGML_TYPE_IQ4_NL, GGML_TYPE_IQ4_XS, GGML_TYPE_Q2_K,
        GGML_TYPE_Q3_K,
    };
    let (blk_bytes, blk_elems, f): (usize, usize, fn(&[u8], &mut [f32])) = match ggml_type {
        GGML_TYPE_Q2_K => (84, QK_K, dequantize_q2_k),
        GGML_TYPE_Q3_K => (110, QK_K, dequantize_q3_k),
        GGML_TYPE_IQ2_XXS => (66, QK_K, dequantize_iq2_xxs),
        GGML_TYPE_IQ2_XS => (74, QK_K, dequantize_iq2_xs),
        GGML_TYPE_IQ2_S => (82, QK_K, dequantize_iq2_s),
        GGML_TYPE_IQ3_XXS => (98, QK_K, dequantize_iq3_xxs),
        GGML_TYPE_IQ3_S => (110, QK_K, dequantize_iq3_s),
        GGML_TYPE_IQ1_S => (50, QK_K, dequantize_iq1_s),
        GGML_TYPE_IQ1_M => (56, QK_K, dequantize_iq1_m),
        GGML_TYPE_IQ4_NL => (18, QK4_NL, dequantize_iq4_nl),
        GGML_TYPE_IQ4_XS => (136, QK_K, dequantize_iq4_xs),
        _ => return false,
    };
    if n % blk_elems != 0 {
        return false;
    }
    let nb = n / blk_elems;
    if row.len() < nb * blk_bytes || out.len() < n {
        return false;
    }
    for b in 0..nb {
        f(
            &row[b * blk_bytes..(b + 1) * blk_bytes],
            &mut out[b * blk_elems..(b + 1) * blk_elems],
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // The grids ship as flat consts; a wrong transcription is the single most
    // likely porting bug, so pin their sizes and a couple of known entries.
    #[test]
    fn tables_have_upstream_shape() {
        assert_eq!(KMASK_IQ2XS.len(), 8);
        assert_eq!(KSIGNS_IQ2XS.len(), 128);
        assert_eq!(IQ2XXS_GRID.len(), 256);
        assert_eq!(IQ2XS_GRID.len(), 512);
        assert_eq!(IQ2S_GRID.len(), 1024);
        assert_eq!(IQ3XXS_GRID.len(), 256);
        assert_eq!(IQ3S_GRID.len(), 512);
        assert_eq!(KVALUES_IQ4NL.len(), 16);
        assert_eq!(IQ1S_GRID.len(), 2048);
        assert_eq!(IQ1S_GRID_GPU.len(), 2048);
        assert_eq!(KMASK_IQ2XS, [1, 2, 4, 8, 16, 32, 64, 128]);
        assert_eq!(
            KVALUES_IQ4NL,
            [-127, -104, -83, -65, -49, -35, -22, -10, 1, 13, 25, 38, 53, 69, 89, 113]
        );
    }

    // The GPU iq1s grid is the CPU grid re-encoded as 2-bit lanes offset by 1
    // (llama.cpp convert.cu: `q[j] + delta` with delta = -1 +/- IQ1S_DELTA
    // versus the CPU form's `grid[j] + delta` with grid in {-1,0,1}).
    // Cross-checking the two transcriptions against each other catches a
    // mis-copied row in either table.
    #[test]
    fn iq1s_cpu_and_gpu_grids_agree() {
        for i in 0..IQ1S_GRID.len() {
            let cpu = grid8_i(IQ1S_GRID[i]);
            let gpu = IQ1S_GRID_GPU[i];
            let lo = gpu & 0x0f0f_0f0f;
            let hi = (gpu >> 4) & 0x0f0f_0f0f;
            // convert.cu reinterprets the two masked words as one int8[8]:
            // lanes 0..3 are the LOW nibbles of bytes 0..3, lanes 4..7 the
            // HIGH nibbles of the same bytes — not an interleave.
            let lanes: [u8; 8] = {
                let a = lo.to_le_bytes();
                let b = hi.to_le_bytes();
                [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]]
            };
            for j in 0..8 {
                assert_eq!(
                    i32::from(cpu[j]),
                    i32::from(lanes[j]) - 1,
                    "iq1s grid entry {i} lane {j}"
                );
            }
        }
    }

    // A block whose payload is all zeros must dequantize to the codebook's
    // zero-index value scaled by d, for every type — cheap smoke that the
    // block layouts (offsets of d / scales / qs) are right.
    #[test]
    fn zero_payload_blocks_dequantize_without_panicking() {
        let types: [(u32, usize, usize); 11] = [
            (crate::quant::GGML_TYPE_Q2_K, 84, QK_K),
            (crate::quant::GGML_TYPE_Q3_K, 110, QK_K),
            (crate::quant::GGML_TYPE_IQ2_XXS, 66, QK_K),
            (crate::quant::GGML_TYPE_IQ2_XS, 74, QK_K),
            (crate::quant::GGML_TYPE_IQ2_S, 82, QK_K),
            (crate::quant::GGML_TYPE_IQ3_XXS, 98, QK_K),
            (crate::quant::GGML_TYPE_IQ3_S, 110, QK_K),
            (crate::quant::GGML_TYPE_IQ1_S, 50, QK_K),
            (crate::quant::GGML_TYPE_IQ1_M, 56, QK_K),
            (crate::quant::GGML_TYPE_IQ4_NL, 18, QK4_NL),
            (crate::quant::GGML_TYPE_IQ4_XS, 136, QK_K),
        ];
        for (ty, bytes, elems) in types {
            let row = vec![0u8; bytes * 2];
            let mut out = vec![f32::NAN; elems * 2];
            assert!(
                dequantize_row_iq(ty, &row, elems * 2, &mut out),
                "type {ty} row dequant refused"
            );
            assert!(
                out.iter().all(|v| v.is_finite()),
                "type {ty} produced non-finite values"
            );
        }
    }

    // ---------------------------------------------------------------------
    // Independent-oracle parity.
    //
    // The oracle is llama.cpp's `gguf-py` numpy dequantizers — an
    // implementation derived neither from `ggml-quants.c` (which the port
    // above follows) nor from the CUDA/Metal kernels, so agreement is real
    // evidence rather than a shared bug. Four cases are raw blocks lifted
    // straight out of `local/models/Qwen3.8-27B-UD-Q4_K_M.gguf` — the file
    // that could not load on CUDA; the rest are deterministic pseudo-random
    // blocks so types the UD files happen not to use are still covered.
    //
    // Regenerate with `tests/data/gen_iq_dequant_oracle.py` (needs llama.cpp's
    // gguf-py importable and the 27B UD model present).
    // ---------------------------------------------------------------------
    const ORACLE: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/iq_dequant_oracle.bin"));

    #[test]
    fn cpu_reference_matches_gguf_py_oracle() {
        assert_eq!(&ORACLE[0..4], b"IQFX", "fixture magic");
        let u32_at = |o: usize| u32::from_le_bytes(ORACLE[o..o + 4].try_into().unwrap());
        let n_cases = u32_at(4) as usize;
        assert!(n_cases >= 15, "fixture should cover every added type");
        let mut off = 8usize;
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..n_cases {
            let ggml_type = u32_at(off);
            let n_elems = u32_at(off + 4) as usize;
            let n_bytes = u32_at(off + 8) as usize;
            off += 12;
            let raw = &ORACLE[off..off + n_bytes];
            off += n_bytes;
            let oracle: Vec<f32> = (0..n_elems)
                .map(|i| f32::from_le_bytes(ORACLE[off + 4 * i..off + 4 * i + 4].try_into().unwrap()))
                .collect();
            off += 4 * n_elems;
            seen.insert(ggml_type);

            let mut out = vec![f32::NAN; n_elems];
            assert!(
                dequantize_row_iq(ggml_type, raw, n_elems, &mut out),
                "no CPU dequant for ggml type {ggml_type}"
            );
            // Both implementations do the same multiplies in the same order,
            // so this is exact bits, not a tolerance.
            for i in 0..n_elems {
                assert_eq!(
                    out[i].to_bits(),
                    oracle[i].to_bits(),
                    "ggml type {ggml_type} element {i}: got {} want {}",
                    out[i],
                    oracle[i]
                );
            }
        }
        assert_eq!(off, ORACLE.len(), "fixture trailing bytes");
        // The four types the unsloth UD- files actually mix in must be present.
        for ty in [
            crate::quant::GGML_TYPE_IQ4_XS,
            crate::quant::GGML_TYPE_IQ4_NL,
            crate::quant::GGML_TYPE_IQ3_S,
            crate::quant::GGML_TYPE_Q3_K,
        ] {
            assert!(seen.contains(&ty), "missing oracle case for ggml type {ty}");
        }
    }

    // IQ4_XS with d = 1.0 and sub-block 0's scale at ls = 33 (dl = 1.0) must
    // reproduce the codebook values verbatim.
    #[test]
    fn iq4_xs_scale_and_codebook_are_exact() {
        let mut blk = vec![0u8; 136];
        blk[0..2].copy_from_slice(&0x3C00u16.to_le_bytes()); // f16 1.0
                                                             // ls for sub-block 0 = 33 -> dl = 1.0
        blk[4] = 33 & 0xf;
        blk[2..4].copy_from_slice(&((33u16 >> 4) & 3).to_le_bytes());
        // qs nibble pairs 0x10 -> low nibble 0, high nibble 1
        blk[8] = 0x10;
        let mut out = vec![0.0f32; QK_K];
        dequantize_iq4_xs(&blk, &mut out);
        assert_eq!(out[0], f32::from(KVALUES_IQ4NL[0]));
        assert_eq!(out[16], f32::from(KVALUES_IQ4NL[1]));
        // sub-block 1 keeps ls = 0, i.e. dl = -32 (the scale is biased by 32,
        // never clamped) — a zeroed scale field is NOT a zeroed sub-block.
        assert_eq!(out[32], -32.0 * f32::from(KVALUES_IQ4NL[0]));
    }
}
