// Quantization block formats matching GGML's layout.
// All blocks quantize 32 elements (QK=32).

pub const QK: usize = 32;
pub const QK_NVFP4_SUB: usize = 16;

/// Convert f16 (IEEE 754 half-precision) to f32.
#[inline]
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;

    if exp == 0 {
        if mant == 0 {
            return f32::from_bits(sign << 31);
        }
        // subnormal
        let mut e = 0i32;
        let mut m = mant;
        while (m & 0x400) == 0 {
            m <<= 1;
            e -= 1;
        }
        m &= 0x3ff;
        let exp32 = (127 - 15 + 1 + e) as u32;
        return f32::from_bits((sign << 31) | (exp32 << 23) | (m << 13));
    }
    if exp == 31 {
        if mant == 0 {
            return f32::from_bits((sign << 31) | (0xff << 23));
        }
        return f32::from_bits((sign << 31) | (0xff << 23) | (mant << 13));
    }
    let exp32 = exp + (127 - 15);
    f32::from_bits((sign << 31) | (exp32 << 23) | (mant << 13))
}

/// Convert f32 to f16.
#[inline]
pub fn f32_to_f16(f: f32) -> u16 {
    let b = f.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let mant = b & 0x7fffff;

    if exp == 0xff {
        if mant == 0 {
            return sign | 0x7c00;
        }
        return sign | 0x7c00 | ((mant >> 13) as u16).max(1);
    }
    let unbiased = exp - 127;
    if unbiased > 15 {
        return sign | 0x7c00;
    }
    if unbiased < -14 {
        // subnormal or zero
        if unbiased < -24 {
            return sign;
        }
        let shift = (-1 - unbiased) as u32;
        let m = (0x800000 | mant) >> (shift + 1);
        // round to nearest even
        let round = (m >> 13) as u16;
        return sign | round;
    }
    let h_exp = ((unbiased + 15) as u16) << 10;
    let h_mant = (mant >> 13) as u16;
    sign | h_exp | h_mant
}

/// Convert f32 to f16 with round-to-nearest-even (the IEEE default, matching
/// device `__float2half` and torch `.half()`).  [`f32_to_f16`] above
/// truncates; existing weight caches lock that behavior, so new consumers
/// that need torch-parity casts opt into this one explicitly.
#[inline]
pub fn f32_to_f16_rn(f: f32) -> u16 {
    let b = f.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let mant = b & 0x007f_ffff;
    if exp == 0xff {
        if mant == 0 {
            return sign | 0x7c00;
        }
        return sign | 0x7c00 | ((mant >> 13) as u16).max(1);
    }
    let unbiased = exp - 127;
    if unbiased >= 16 {
        return sign | 0x7c00;
    }
    if unbiased < -14 {
        // Subnormal or zero: shift the 24-bit significand so ties land on
        // the halfway bit, then round to nearest even.
        if unbiased < -25 {
            return sign;
        }
        let m = 0x0080_0000 | mant;
        let shift = (-1 - unbiased) as u32;
        let halfway = 1u32 << (shift - 1);
        let rem = m & ((1u32 << shift) - 1);
        let mut v = m >> shift;
        if rem > halfway || (rem == halfway && v & 1 == 1) {
            v += 1;
        }
        return sign | v as u16;
    }
    let mut h = (((unbiased + 15) as u32) << 10) | (mant >> 13);
    let rem = mant & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && h & 1 == 1) {
        h += 1;
    }
    // A carry out of the top mantissa/exponent bit is correct: it walks
    // into the next binade, and 65520.0 upward becomes infinity like RNE
    // demands.
    sign | h as u16
}

/// Convert bf16 to f32.
#[inline]
pub fn bf16_to_f32(b: u16) -> f32 {
    f32::from_bits((b as u32) << 16)
}

/// Convert ggml's UE4M3 byte encoding to f32.
#[inline]
pub fn ue4m3_to_f32(x: u8) -> f32 {
    if x == 0 || x == 0x7f || x == 0xff {
        return 0.0;
    }
    let exp = ((x >> 3) & 0x0f) as i32;
    let man = (x & 0x07) as i32;
    let raw = if exp == 0 {
        (man as f32) * 2f32.powi(-9)
    } else {
        (1.0 + man as f32 / 8.0) * 2f32.powi(exp - 7)
    };
    raw * 0.5
}

/// Signed FP8 E4M3FN scalar decode (torch float8_e4m3fn / safetensors
/// "F8_E4M3"): 1 sign + 4 exponent (bias 7) + 3 mantissa bits, implicit
/// scale 1.0. No infinities; 0x7f/0xff decode to NaN — loaders reject those
/// bytes fail-closed. NOT the same as [`ue4m3_to_f32`]/[`e4m3_scale_to_f32`],
/// which are unsigned magnitude helpers for NVFP4 block scales (one bakes in
/// a x0.5 compensation); never use those for signed weight payloads.
/// Anchors: 0x01=2^-9, 0x08=2^-6, 0x38=+1.0, 0xB8=-1.0, 0x7e=+448, 0xfe=-448.
#[inline]
pub fn f8_e4m3_to_f32(x: u8) -> f32 {
    let sign = if x & 0x80 != 0 { -1.0f32 } else { 1.0f32 };
    let exp = ((x >> 3) & 0x0f) as i32;
    let man = (x & 0x07) as i32;
    if exp == 0x0f && man == 0x07 {
        return f32::NAN;
    }
    let magnitude = if exp == 0 {
        // Subnormal: man * 2^-9 (i.e. 2^-6 * man/8).
        (man as f32) * 2f32.powi(-9)
    } else {
        (1.0 + man as f32 / 8.0) * 2f32.powi(exp - 7)
    };
    sign * magnitude
}

const MXFP4_VALUES_X2: [i8; 16] = [0, 1, 2, 3, 4, 6, 8, 12, 0, -1, -2, -3, -4, -6, -8, -12];

// ---- Dequantization for each block type ----

/// Q4_0: 4-bit quantization, block = 2 bytes (f16 scale) + 16 bytes (32 nibbles)
/// Total: 18 bytes per 32 elements
pub fn dequantize_q4_0(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 18);
    debug_assert!(out.len() >= QK);
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let qs = &block[2..18];
    for j in 0..QK / 2 {
        let lo = (qs[j] & 0x0f) as i32 - 8;
        let hi = ((qs[j] >> 4) & 0x0f) as i32 - 8;
        out[j] = lo as f32 * d;
        out[j + QK / 2] = hi as f32 * d;
    }
}

/// Q4_1: 4-bit with min, block = 2 bytes (f16 d) + 2 bytes (f16 m) + 16 bytes
/// Total: 20 bytes per 32 elements
pub fn dequantize_q4_1(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 20);
    debug_assert!(out.len() >= QK);
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let m = f16_to_f32(u16::from_le_bytes([block[2], block[3]]));
    let qs = &block[4..20];
    for j in 0..QK / 2 {
        let lo = (qs[j] & 0x0f) as f32;
        let hi = ((qs[j] >> 4) & 0x0f) as f32;
        out[j] = lo * d + m;
        out[j + QK / 2] = hi * d + m;
    }
}

/// Q5_0: 5-bit quantization, block = 2 bytes (f16 d) + 4 bytes (high bits) + 16 bytes (low nibbles)
/// Total: 22 bytes per 32 elements
pub fn dequantize_q5_0(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 22);
    debug_assert!(out.len() >= QK);
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let qh = u32::from_le_bytes([block[2], block[3], block[4], block[5]]);
    let qs = &block[6..22];
    for j in 0..QK / 2 {
        let xh_0 = ((qh >> j) & 1) as i32;
        let xh_1 = ((qh >> (j + 16)) & 1) as i32;
        let lo = ((qs[j] & 0x0f) as i32 | (xh_0 << 4)) - 16;
        let hi = (((qs[j] >> 4) & 0x0f) as i32 | (xh_1 << 4)) - 16;
        out[j] = lo as f32 * d;
        out[j + QK / 2] = hi as f32 * d;
    }
}

/// Q5_1: 5-bit with min, block = 2+2 bytes (f16 d,m) + 4 bytes (high bits) + 16 bytes
/// Total: 24 bytes per 32 elements
pub fn dequantize_q5_1(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 24);
    debug_assert!(out.len() >= QK);
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let m = f16_to_f32(u16::from_le_bytes([block[2], block[3]]));
    let qh = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    let qs = &block[8..24];
    for j in 0..QK / 2 {
        let xh_0 = ((qh >> j) & 1) as u32;
        let xh_1 = ((qh >> (j + 16)) & 1) as u32;
        let lo = (qs[j] & 0x0f) as u32 | (xh_0 << 4);
        let hi = ((qs[j] >> 4) & 0x0f) as u32 | (xh_1 << 4);
        out[j] = lo as f32 * d + m;
        out[j + QK / 2] = hi as f32 * d + m;
    }
}

/// Q8_0: 8-bit quantization, block = 2 bytes (f16 d) + 32 bytes (int8)
/// Total: 34 bytes per 32 elements
pub fn dequantize_q8_0(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 34);
    debug_assert!(out.len() >= QK);
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    for j in 0..QK {
        out[j] = (block[2 + j] as i8) as f32 * d;
    }
}

/// NVFP4: 4 UE4M3 sub-block scales + 32 packed 4-bit E2M1 values for 64 elements.
pub fn dequantize_nvfp4(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 36);
    debug_assert!(out.len() >= QK_NVFP4);
    let scales = &block[..QK_NVFP4 / QK_NVFP4_SUB];
    let qs = &block[QK_NVFP4 / QK_NVFP4_SUB..36];
    for (sub, &scale_byte) in scales.iter().enumerate() {
        let d = ue4m3_to_f32(scale_byte);
        let yb = &mut out[sub * QK_NVFP4_SUB..(sub + 1) * QK_NVFP4_SUB];
        let qb = &qs[sub * (QK_NVFP4_SUB / 2)..(sub + 1) * (QK_NVFP4_SUB / 2)];
        for (j, &packed) in qb.iter().enumerate() {
            let v0 = MXFP4_VALUES_X2[(packed & 0x0f) as usize] as f32;
            let v1 = MXFP4_VALUES_X2[(packed >> 4) as usize] as f32;
            yb[j] = d * v0;
            yb[j + QK_NVFP4_SUB / 2] = d * v1;
        }
    }
}

/// Dot product: one NVFP4 block (64 quantized values) dot 64 f32 values.
pub fn vec_dot_nvfp4_f32(block: &[u8], v: &[f32]) -> f32 {
    debug_assert!(block.len() >= 36);
    debug_assert!(v.len() >= QK_NVFP4);
    let scales = &block[..QK_NVFP4 / QK_NVFP4_SUB];
    let qs = &block[QK_NVFP4 / QK_NVFP4_SUB..36];
    let mut sum = 0.0f32;
    for (sub, &scale_byte) in scales.iter().enumerate() {
        let d = ue4m3_to_f32(scale_byte);
        let xb = &v[sub * QK_NVFP4_SUB..(sub + 1) * QK_NVFP4_SUB];
        let qb = &qs[sub * (QK_NVFP4_SUB / 2)..(sub + 1) * (QK_NVFP4_SUB / 2)];
        let mut sub_sum = 0.0f32;
        for (j, &packed) in qb.iter().enumerate() {
            let v0 = MXFP4_VALUES_X2[(packed & 0x0f) as usize] as f32;
            let v1 = MXFP4_VALUES_X2[(packed >> 4) as usize] as f32;
            sub_sum += v0 * xb[j];
            sub_sum += v1 * xb[j + QK_NVFP4_SUB / 2];
        }
        sum += d * sub_sum;
    }
    sum
}

// ---- NVFP4 "pairs" (ComfyUI / TensorRT-ModelOpt safetensors layout) ----
//
// Unlike the GGML NVFP4 super-block (36 bytes interleaved), checkpoints such
// as the MiniMax-H3 NVFP4 repacks store one quantized linear as three (or
// four) tensors kept verbatim on device:
//   weight        u8  [rows, cols/2]   two E2M1 nibbles per byte, SEQUENTIAL
//                                      order (low nibble = even element)
//   weight_scale  u8  [rows, cols/16]  E4M3 per-16 block scales (magnitude)
//   weight_scale_2 f32 scalar          per-tensor global scale
//   pre_quant_scale bf16 [cols]        optional AWQ input smoothing; folded
//                                      into the dequantized weight as a
//                                      per-input-column multiply
// `dequantized = scale_2 * e4m3(scale) * e2m1(nibble) [* pre_scale[col]]`.

/// Local ggml-type extension ids for the pairs layout (see
/// `h3_nvfp4_pairs_bytes` for the packed device blob these describe). Kept
/// far outside the upstream ggml enum range; `block_size`/`block_elements`
/// intentionally do NOT cover them — the layout is not block-uniform.
pub const GGML_TYPE_H3_NVFP4_PAIRS: u32 = 990;
pub const GGML_TYPE_H3_NVFP4_PAIRS_PRESCALE: u32 = 991;

/// Byte-offset header of the packed pairs blob uploaded to the device:
/// `[H3NvFp4PairsHeader | scales u8[rows*cols/16] | weights u8[rows*cols/2]
///  | pre_scale bf16[cols] (PRESCALE only)]`.
pub const H3_NVFP4_PAIRS_HEADER_BYTES: usize = 32;
pub const H3_NVFP4_PAIRS_MAGIC: u32 = 0x4E56_3450; // "NV4P"

/// Total packed blob size for an `(rows, cols)` pairs tensor.
pub fn h3_nvfp4_pairs_bytes(rows: usize, cols: usize, has_pre_scale: bool) -> Option<usize> {
    if cols % 16 != 0 || cols == 0 || rows == 0 {
        return None;
    }
    let scales = rows.checked_mul(cols / 16)?;
    let weights = rows.checked_mul(cols / 2)?;
    let pre = if has_pre_scale { cols.checked_mul(2)? } else { 0 };
    H3_NVFP4_PAIRS_HEADER_BYTES
        .checked_add(scales)?
        .checked_add(weights)?
        .checked_add(pre)
}

/// Builds the packed pairs blob (header + sections) from the checkpoint's
/// raw tensor bytes. `pre_scale_bf16` are the raw bf16 words when present.
pub fn h3_nvfp4_pairs_pack(
    rows: usize,
    cols: usize,
    scale2: f32,
    scale_bytes: &[u8],
    weight_bytes: &[u8],
    pre_scale_bf16: Option<&[u8]>,
) -> Option<Vec<u8>> {
    let total = h3_nvfp4_pairs_bytes(rows, cols, pre_scale_bf16.is_some())?;
    if scale_bytes.len() != rows * (cols / 16) || weight_bytes.len() != rows * (cols / 2) {
        return None;
    }
    if let Some(pre) = pre_scale_bf16 {
        if pre.len() != cols * 2 {
            return None;
        }
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&H3_NVFP4_PAIRS_MAGIC.to_le_bytes());
    out.extend_from_slice(&(rows as u32).to_le_bytes());
    out.extend_from_slice(&(cols as u32).to_le_bytes());
    out.extend_from_slice(&(pre_scale_bf16.is_some() as u32).to_le_bytes());
    out.extend_from_slice(&scale2.to_le_bytes());
    out.extend_from_slice(&[0u8; 12]);
    out.extend_from_slice(scale_bytes);
    out.extend_from_slice(weight_bytes);
    if let Some(pre) = pre_scale_bf16 {
        out.extend_from_slice(pre);
    }
    debug_assert_eq!(out.len(), total);
    Some(out)
}

/// Plain E2M1 magnitudes (the ×2 LUT above is a dp4a trick paired with the
/// halved `ue4m3_to_f32`; the pairs path keeps both factors undoctored).
pub const E2M1_VALUES: [f32; 16] = [
    0.0, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, -0.0, -0.5, -1.0, -1.5, -2.0, -3.0, -4.0, -6.0,
];

/// E4M3 scale byte decode without the ×0.5 compensation baked into
/// `ue4m3_to_f32` (sign bit ignored — ModelOpt block scales are magnitudes).
pub fn e4m3_scale_to_f32(x: u8) -> f32 {
    ue4m3_to_f32(x) * 2.0
}

/// CPU reference decode of one pairs row (`cols` values). `pre_scale` is the
/// optional per-column AWQ multiplier already decoded to f32.
pub fn dequantize_nvfp4_pairs_row(
    weight_row: &[u8],
    scale_row: &[u8],
    scale2: f32,
    pre_scale: Option<&[f32]>,
    out: &mut [f32],
) {
    let cols = out.len();
    debug_assert_eq!(weight_row.len(), cols / 2);
    debug_assert_eq!(scale_row.len(), cols / 16);
    for (group, &scale_byte) in scale_row.iter().enumerate() {
        let d = scale2 * e4m3_scale_to_f32(scale_byte);
        for j in 0..8 {
            let packed = weight_row[group * 8 + j];
            let col0 = group * 16 + 2 * j;
            let mut v0 = d * E2M1_VALUES[(packed & 0x0f) as usize];
            let mut v1 = d * E2M1_VALUES[(packed >> 4) as usize];
            if let Some(pre) = pre_scale {
                v0 *= pre[col0];
                v1 *= pre[col0 + 1];
            }
            out[col0] = v0;
            out[col0 + 1] = v1;
        }
    }
}

/// Gather row-major GGML tensor rows into dequantized f32 output on CPU.
pub fn get_rows_ggml_bytes_cpu(
    src: &[u8],
    src_ggml_type: u32,
    n_cols: usize,
    n_rows: usize,
    row_indices: &[i32],
) -> Option<Vec<f32>> {
    if row_indices.is_empty() {
        return Some(Vec::new());
    }

    let row_bytes = match src_ggml_type {
        GGML_TYPE_F32 => n_cols.checked_mul(4)?,
        GGML_TYPE_F16 | GGML_TYPE_BF16 => n_cols.checked_mul(2)?,
        GGML_TYPE_F8_E4M3 => n_cols,
        GGML_TYPE_Q4_0 | GGML_TYPE_Q4_1 | GGML_TYPE_Q5_0 | GGML_TYPE_Q5_1 | GGML_TYPE_Q8_0
        | GGML_TYPE_Q4_K | GGML_TYPE_Q5_K | GGML_TYPE_Q6_K | GGML_TYPE_NVFP4 => {
            let block_elems = block_elements(src_ggml_type);
            if n_cols % block_elems != 0 {
                return None;
            }
            (n_cols / block_elems).checked_mul(block_size(src_ggml_type))?
        }
        _ => return None,
    };

    if src.len() != n_rows.checked_mul(row_bytes)? {
        return None;
    }

    let mut out = Vec::with_capacity(n_cols.checked_mul(row_indices.len())?);
    for &row in row_indices {
        let row = usize::try_from(row).ok()?;
        if row >= n_rows {
            return None;
        }
        let row_src = &src[row * row_bytes..(row + 1) * row_bytes];
        match src_ggml_type {
            GGML_TYPE_F32 => {
                for chunk in row_src.chunks_exact(4) {
                    out.push(f32::from_le_bytes(chunk.try_into().unwrap()));
                }
            }
            GGML_TYPE_F16 => {
                for chunk in row_src.chunks_exact(2) {
                    out.push(f16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())));
                }
            }
            GGML_TYPE_BF16 => {
                for chunk in row_src.chunks_exact(2) {
                    out.push(bf16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())));
                }
            }
            GGML_TYPE_F8_E4M3 => {
                for &byte in row_src {
                    out.push(f8_e4m3_to_f32(byte));
                }
            }
            GGML_TYPE_Q4_0 => dequantize_row_blocks(row_src, n_cols, 18, dequantize_q4_0, &mut out),
            GGML_TYPE_Q4_1 => dequantize_row_blocks(row_src, n_cols, 20, dequantize_q4_1, &mut out),
            GGML_TYPE_Q5_0 => dequantize_row_blocks(row_src, n_cols, 22, dequantize_q5_0, &mut out),
            GGML_TYPE_Q5_1 => dequantize_row_blocks(row_src, n_cols, 24, dequantize_q5_1, &mut out),
            GGML_TYPE_Q8_0 => dequantize_row_blocks(row_src, n_cols, 34, dequantize_q8_0, &mut out),
            GGML_TYPE_Q4_K => dequantize_row_q4_k(row_src, n_cols, &mut out),
            GGML_TYPE_Q5_K => dequantize_row_q5_k(row_src, n_cols, &mut out),
            GGML_TYPE_Q6_K => dequantize_row_q6_k(row_src, n_cols, &mut out),
            GGML_TYPE_NVFP4 => {
                dequantize_row_nvfp4(row_src, n_cols, &mut out);
            }
            _ => return None,
        }
    }
    Some(out)
}

fn dequantize_row_blocks(
    row_src: &[u8],
    n_cols: usize,
    block_bytes: usize,
    dequantize: fn(&[u8], &mut [f32]),
    out: &mut Vec<f32>,
) {
    debug_assert_eq!(n_cols % QK, 0);
    debug_assert_eq!(row_src.len(), (n_cols / QK) * block_bytes);
    let mut block_out = [0.0f32; QK];
    for block in row_src.chunks_exact(block_bytes) {
        dequantize(block, &mut block_out);
        out.extend_from_slice(&block_out);
    }
}

/// q4_K super-block: `d` f16, `dmin` f16, 12 bytes of 6-bit packed
/// scales/mins, 128 bytes of 4-bit quants — 144 bytes for 256 values.
/// `y = d*sc*q - dmin*m` per 32-value group, exactly upstream ggml
/// `dequantize_row_q4_K` (low nibbles then high nibbles per 64-value pair).
pub fn dequantize_q4_k(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 144);
    debug_assert!(out.len() >= QK_K);
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let dmin = f16_to_f32(u16::from_le_bytes([block[2], block[3]]));
    let scales = &block[4..16];
    let qs = &block[16..144];
    let mut is = 0usize;
    for j in (0..QK_K).step_by(64) {
        let q = &qs[32 * (j / 64)..32 * (j / 64) + 32];
        let (sc1, m1) = get_scale_min_k4(is, scales);
        let (sc2, m2) = get_scale_min_k4(is + 1, scales);
        let d1 = d * sc1 as f32;
        let min1 = dmin * m1 as f32;
        let d2 = d * sc2 as f32;
        let min2 = dmin * m2 as f32;
        for l in 0..32 {
            out[j + l] = d1 * (q[l] & 0x0F) as f32 - min1;
        }
        for l in 0..32 {
            out[j + 32 + l] = d2 * (q[l] >> 4) as f32 - min2;
        }
        is += 2;
    }
}

fn dequantize_row_q4_k(row_src: &[u8], n_cols: usize, out: &mut Vec<f32>) {
    debug_assert_eq!(n_cols % QK_K, 0);
    debug_assert_eq!(row_src.len(), (n_cols / QK_K) * block_size(GGML_TYPE_Q4_K));
    let mut block_out = [0.0f32; QK_K];
    for block in row_src.chunks_exact(block_size(GGML_TYPE_Q4_K)) {
        dequantize_q4_k(block, &mut block_out);
        out.extend_from_slice(&block_out);
    }
}

/// q6_K super-block: 128 bytes low quants, 64 bytes high bits, 16 signed
/// scales, trailing `d` f16 — 210 bytes for 256 values. `y = d*sc*(q-32)`,
/// exactly upstream ggml `dequantize_row_q6_K`.
pub fn dequantize_q6_k(block: &[u8], out: &mut [f32]) {
    debug_assert!(block.len() >= 210);
    debug_assert!(out.len() >= QK_K);
    let d = f16_to_f32(u16::from_le_bytes([block[208], block[209]]));
    for n in 0..2 {
        let ql = &block[n * 64..];
        let qh = &block[128 + n * 32..];
        let sc = &block[192 + n * 8..];
        let y = &mut out[n * 128..];
        for l in 0..32 {
            let is = l / 16;
            let q1 = (((ql[l] & 0x0F) | ((qh[l] & 3) << 4)) as i8 as i32) - 32;
            let q2 = (((ql[l + 32] & 0x0F) | (((qh[l] >> 2) & 3) << 4)) as i8 as i32) - 32;
            let q3 = (((ql[l] >> 4) | (((qh[l] >> 4) & 3) << 4)) as i8 as i32) - 32;
            let q4 = (((ql[l + 32] >> 4) | (((qh[l] >> 6) & 3) << 4)) as i8 as i32) - 32;
            y[l] = d * (sc[is] as i8 as f32) * q1 as f32;
            y[l + 32] = d * (sc[is + 2] as i8 as f32) * q2 as f32;
            y[l + 64] = d * (sc[is + 4] as i8 as f32) * q3 as f32;
            y[l + 96] = d * (sc[is + 6] as i8 as f32) * q4 as f32;
        }
    }
}

fn dequantize_row_q6_k(row_src: &[u8], n_cols: usize, out: &mut Vec<f32>) {
    debug_assert_eq!(n_cols % QK_K, 0);
    debug_assert_eq!(row_src.len(), (n_cols / QK_K) * block_size(GGML_TYPE_Q6_K));
    let mut block_out = [0.0f32; QK_K];
    for block in row_src.chunks_exact(block_size(GGML_TYPE_Q6_K)) {
        dequantize_q6_k(block, &mut block_out);
        out.extend_from_slice(&block_out);
    }
}

fn dequantize_row_q5_k(row_src: &[u8], n_cols: usize, out: &mut Vec<f32>) {
    debug_assert_eq!(n_cols % QK_K, 0);
    debug_assert_eq!(row_src.len(), (n_cols / QK_K) * block_size(GGML_TYPE_Q5_K));

    for block in row_src.chunks_exact(block_size(GGML_TYPE_Q5_K)) {
        let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
        let dmin = f16_to_f32(u16::from_le_bytes([block[2], block[3]]));
        let scales = &block[4..16];
        let qh = &block[16..48];
        let qs = &block[48..176];

        let mut is = 0usize;
        let mut u1 = 1u8;
        let mut u2 = 2u8;
        let mut ql_offset = 0usize;
        for _ in 0..4 {
            let (sc1, m1) = get_scale_min_k4(is + 0, scales);
            let (sc2, m2) = get_scale_min_k4(is + 1, scales);
            let d1 = d * sc1 as f32;
            let d2 = d * sc2 as f32;
            let m1 = dmin * m1 as f32;
            let m2 = dmin * m2 as f32;
            let ql = &qs[ql_offset..ql_offset + 32];
            for l in 0..32 {
                out.push(
                    d1 * (((ql[l] & 0x0F) as f32) + if (qh[l] & u1) != 0 { 16.0 } else { 0.0 })
                        - m1,
                );
            }
            for l in 0..32 {
                out.push(
                    d2 * (((ql[l] >> 4) as f32) + if (qh[l] & u2) != 0 { 16.0 } else { 0.0 }) - m2,
                );
            }
            ql_offset += 32;
            is += 2;
            u1 <<= 2;
            u2 <<= 2;
        }
    }
}

fn dequantize_row_nvfp4(row_src: &[u8], n_cols: usize, out: &mut Vec<f32>) {
    debug_assert_eq!(n_cols % QK_NVFP4, 0);
    debug_assert_eq!(
        row_src.len(),
        (n_cols / QK_NVFP4) * block_size(GGML_TYPE_NVFP4)
    );
    let mut block_out = [0.0f32; QK_NVFP4];
    for block in row_src.chunks_exact(block_size(GGML_TYPE_NVFP4)) {
        dequantize_nvfp4(block, &mut block_out);
        out.extend_from_slice(&block_out);
    }
}

fn get_scale_min_k4(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        (
            (q[j + 4] & 0x0F) | ((q[j - 4] >> 6) << 4),
            (q[j + 4] >> 4) | ((q[j] >> 6) << 4),
        )
    }
}

// ---- Vector dot products (quantized × f32 → partial sum) ----
// Used in matrix multiply: dot product of a quantized row with an f32 row

/// Dot product: one Q4_0 block (32 quantized values) dot 32 f32 values
pub fn vec_dot_q4_0_f32(block: &[u8], v: &[f32]) -> f32 {
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let qs = &block[2..18];
    let mut sum = 0.0f32;
    for j in 0..QK / 2 {
        let lo = (qs[j] & 0x0f) as i32 - 8;
        let hi = ((qs[j] >> 4) & 0x0f) as i32 - 8;
        sum += lo as f32 * v[j];
        sum += hi as f32 * v[j + QK / 2];
    }
    sum * d
}

/// Dot product: one Q5_0 block dot 32 f32 values
pub fn vec_dot_q5_0_f32(block: &[u8], v: &[f32]) -> f32 {
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let qh = u32::from_le_bytes([block[2], block[3], block[4], block[5]]);
    let qs = &block[6..22];
    let mut sum = 0.0f32;
    for j in 0..QK / 2 {
        let xh_0 = ((qh >> j) & 1) as i32;
        let xh_1 = ((qh >> (j + 16)) & 1) as i32;
        let lo = ((qs[j] & 0x0f) as i32 | (xh_0 << 4)) - 16;
        let hi = (((qs[j] >> 4) & 0x0f) as i32 | (xh_1 << 4)) - 16;
        sum += lo as f32 * v[j];
        sum += hi as f32 * v[j + QK / 2];
    }
    sum * d
}

/// Dot product: one Q5_0 block dot one Q8_0 block (32 values each).
/// Returns dequantized f32 sum.
#[inline]
pub fn vec_dot_q5_0_q8_0(a: &[u8], b: &[u8]) -> f32 {
    let da = f16_to_f32(u16::from_le_bytes([a[0], a[1]]));
    let db = f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
    let qh = u32::from_le_bytes([a[2], a[3], a[4], a[5]]);
    let qs = &a[6..22];
    let y = &b[2..34];

    let mut sumi = 0i32;
    for j in 0..QK / 2 {
        let xh_0 = ((qh >> j) & 1) as i32;
        let xh_1 = ((qh >> (j + 16)) & 1) as i32;
        let x0 = ((qs[j] & 0x0f) as i32 | (xh_0 << 4)) - 16;
        let x1 = (((qs[j] >> 4) & 0x0f) as i32 | (xh_1 << 4)) - 16;
        sumi += x0 * (y[j] as i8 as i32);
        sumi += x1 * (y[j + QK / 2] as i8 as i32);
    }

    (sumi as f32) * da * db
}

/// Dot product: one Q8_0 block dot 32 f32 values
#[inline]
pub fn vec_dot_q8_0_f32(block: &[u8], v: &[f32]) -> f32 {
    vec_dot_q8_0_f32_simd(block, v)
}

/// Dot product: one Q8_0 block dot another Q8_0 block (32 values each).
/// Returns dequantized f32 sum.
#[inline]
pub fn vec_dot_q8_0_q8_0(a: &[u8], b: &[u8]) -> f32 {
    let da = f16_to_f32(u16::from_le_bytes([a[0], a[1]]));
    let db = f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
    let sum = vec_dot_q8_0_q8_0_i32(&a[2..34], &b[2..34]);
    (sum as f32) * da * db
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn vec_dot_q8_0_q8_0_i32(qa: &[u8], qb: &[u8]) -> i32 {
    unsafe { vec_dot_q8_0_q8_0_i32_neon_mul(qa, qb) }
}

#[cfg(target_arch = "aarch64")]
unsafe fn vec_dot_q8_0_q8_0_i32_neon_mul(qa: &[u8], qb: &[u8]) -> i32 {
    use std::arch::aarch64::*;
    let mut acc = vdupq_n_s32(0);
    for i in (0..QK).step_by(16) {
        let a8 = vld1q_s8(qa.as_ptr().add(i) as *const i8);
        let b8 = vld1q_s8(qb.as_ptr().add(i) as *const i8);

        let a_lo = vmovl_s8(vget_low_s8(a8));
        let b_lo = vmovl_s8(vget_low_s8(b8));
        let p_lo = vmulq_s16(a_lo, b_lo);

        let a_hi = vmovl_s8(vget_high_s8(a8));
        let b_hi = vmovl_s8(vget_high_s8(b8));
        let p_hi = vmulq_s16(a_hi, b_hi);

        acc = vaddq_s32(acc, vpaddlq_s16(p_lo));
        acc = vaddq_s32(acc, vpaddlq_s16(p_hi));
    }
    vaddvq_s32(acc)
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn vec_dot_q8_0_q8_0_i32(qa: &[u8], qb: &[u8]) -> i32 {
    let mut sum = 0i32;
    let mut i = 0;
    while i + 3 < QK {
        sum += (qa[i] as i8 as i32) * (qb[i] as i8 as i32);
        sum += (qa[i + 1] as i8 as i32) * (qb[i + 1] as i8 as i32);
        sum += (qa[i + 2] as i8 as i32) * (qb[i + 2] as i8 as i32);
        sum += (qa[i + 3] as i8 as i32) * (qb[i + 3] as i8 as i32);
        i += 4;
    }
    while i < QK {
        sum += (qa[i] as i8 as i32) * (qb[i] as i8 as i32);
        i += 1;
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn vec_dot_q8_0_f32_simd(block: &[u8], v: &[f32]) -> f32 {
    use std::arch::aarch64::*;
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let qs = &block[2..34];
    unsafe {
        let mut sum0 = vdupq_n_f32(0.0);
        let mut sum1 = vdupq_n_f32(0.0);
        let mut sum2 = vdupq_n_f32(0.0);
        let mut sum3 = vdupq_n_f32(0.0);
        // Process 16 elements per iteration, 2 iterations for 32 elements
        for i in (0..32).step_by(16) {
            // Load 16 int8 values and widen to f32
            let q0 = vld1_s8(qs.as_ptr().add(i) as *const i8);
            let q0_16 = vmovl_s8(q0); // i8x8 -> i16x8
            let q0_lo = vmovl_s16(vget_low_s16(q0_16)); // i16x4 -> i32x4
            let q0_hi = vmovl_s16(vget_high_s16(q0_16));
            let w0 = vcvtq_f32_s32(q0_lo);
            let w1 = vcvtq_f32_s32(q0_hi);

            let q1 = vld1_s8(qs.as_ptr().add(i + 8) as *const i8);
            let q1_16 = vmovl_s8(q1);
            let q1_lo = vmovl_s16(vget_low_s16(q1_16));
            let q1_hi = vmovl_s16(vget_high_s16(q1_16));
            let w2 = vcvtq_f32_s32(q1_lo);
            let w3 = vcvtq_f32_s32(q1_hi);

            let v0 = vld1q_f32(v.as_ptr().add(i));
            let v1 = vld1q_f32(v.as_ptr().add(i + 4));
            let v2 = vld1q_f32(v.as_ptr().add(i + 8));
            let v3 = vld1q_f32(v.as_ptr().add(i + 12));

            sum0 = vfmaq_f32(sum0, w0, v0);
            sum1 = vfmaq_f32(sum1, w1, v1);
            sum2 = vfmaq_f32(sum2, w2, v2);
            sum3 = vfmaq_f32(sum3, w3, v3);
        }
        sum0 = vaddq_f32(sum0, sum1);
        sum2 = vaddq_f32(sum2, sum3);
        sum0 = vaddq_f32(sum0, sum2);
        vaddvq_f32(sum0) * d
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn vec_dot_q8_0_f32_simd(block: &[u8], v: &[f32]) -> f32 {
    if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
        unsafe { vec_dot_q8_0_f32_avx2(block, v) }
    } else if is_x86_feature_detected!("avx") && is_x86_feature_detected!("fma") {
        unsafe { vec_dot_q8_0_f32_avx(block, v) }
    } else {
        vec_dot_q8_0_f32_scalar(block, v)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn vec_dot_q8_0_f32_avx2(block: &[u8], v: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let qs_ptr = block.as_ptr().add(2) as *const i8;
    let mut sum = _mm256_setzero_ps();
    for i in (0..32).step_by(8) {
        let q8 = _mm_loadl_epi64(qs_ptr.add(i) as *const __m128i);
        let q32 = _mm256_cvtepi8_epi32(q8);
        let qf = _mm256_cvtepi32_ps(q32);
        let vf = _mm256_loadu_ps(v.as_ptr().add(i));
        sum = _mm256_fmadd_ps(qf, vf, sum);
    }
    let hi = _mm256_extractf128_ps(sum, 1);
    let lo = _mm256_castps256_ps128(sum);
    let sum128 = _mm_add_ps(lo, hi);
    let shuf = _mm_movehdup_ps(sum128);
    let sums = _mm_add_ps(sum128, shuf);
    let shuf2 = _mm_movehl_ps(sums, sums);
    let sums2 = _mm_add_ss(sums, shuf2);
    _mm_cvtss_f32(sums2) * d
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx,fma")]
unsafe fn vec_dot_q8_0_f32_avx(block: &[u8], v: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let qs = &block[2..34];
    let mut sum0 = _mm256_setzero_ps();
    let mut sum1 = _mm256_setzero_ps();
    let mut sum2 = _mm256_setzero_ps();
    let mut sum3 = _mm256_setzero_ps();
    // Process 8 elements at a time, 4 iterations for 32 elements
    for i in (0..32).step_by(8) {
        // Load 8 int8 and convert to f32
        let mut w = [0.0f32; 8];
        for j in 0..8 {
            w[j] = (qs[i + j] as i8) as f32;
        }
        let wv = _mm256_loadu_ps(w.as_ptr());
        let vv = _mm256_loadu_ps(v.as_ptr().add(i));
        match i {
            0 => sum0 = _mm256_fmadd_ps(wv, vv, sum0),
            8 => sum1 = _mm256_fmadd_ps(wv, vv, sum1),
            16 => sum2 = _mm256_fmadd_ps(wv, vv, sum2),
            _ => sum3 = _mm256_fmadd_ps(wv, vv, sum3),
        }
    }
    sum0 = _mm256_add_ps(sum0, sum1);
    sum2 = _mm256_add_ps(sum2, sum3);
    sum0 = _mm256_add_ps(sum0, sum2);
    let hi = _mm256_extractf128_ps(sum0, 1);
    let lo = _mm256_castps256_ps128(sum0);
    let sum128 = _mm_add_ps(lo, hi);
    let shuf = _mm_movehdup_ps(sum128);
    let sums = _mm_add_ps(sum128, shuf);
    let shuf2 = _mm_movehl_ps(sums, sums);
    let sums2 = _mm_add_ss(sums, shuf2);
    _mm_cvtss_f32(sums2) * d
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline]
fn vec_dot_q8_0_f32_simd(block: &[u8], v: &[f32]) -> f32 {
    vec_dot_q8_0_f32_scalar(block, v)
}

#[inline]
#[cfg(not(target_arch = "aarch64"))]
fn vec_dot_q8_0_f32_scalar(block: &[u8], v: &[f32]) -> f32 {
    let d = f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let mut sum = 0.0f32;
    for j in 0..QK {
        sum += (block[2 + j] as i8) as f32 * v[j];
    }
    sum * d
}

pub const QK_K: usize = 256;
pub const QK4_NL: usize = 32;
pub const QK_MXFP4: usize = 32;
pub const QK_NVFP4: usize = 64;

// GGML_TYPE_* dtype ids, `ggml_type_name`, `block_size` and `block_elements`
// moved to makepad-ai-loader (lane T2, /aiarch.md §1) alongside TensorType
// (see tensor.rs) — loader's own formats/gguf.rs needs them and cannot
// depend back on this crate. Re-exported here (both as external paths and,
// via this `pub use`, as names in local scope) so the compute kernels below
// keep resolving GGML_TYPE_*/block_size/block_elements as bare identifiers,
// and every existing `makepad_ai_common::{GGML_TYPE_*, ggml_type_name, block_size,
// block_elements}` call site keeps compiling unchanged.
pub use makepad_ai_loader::quant::{
    block_elements, block_size, ggml_type_name, GGML_TYPE_BF16, GGML_TYPE_COUNT, GGML_TYPE_F16,
    GGML_TYPE_F32, GGML_TYPE_F64, GGML_TYPE_F8_E4M3, GGML_TYPE_I16, GGML_TYPE_I32, GGML_TYPE_I64,
    GGML_TYPE_I8, GGML_TYPE_IQ1_M, GGML_TYPE_IQ1_S, GGML_TYPE_IQ2_S, GGML_TYPE_IQ2_XS,
    GGML_TYPE_IQ2_XXS, GGML_TYPE_IQ3_S, GGML_TYPE_IQ3_XXS, GGML_TYPE_IQ4_NL, GGML_TYPE_IQ4_XS,
    GGML_TYPE_MXFP4, GGML_TYPE_NVFP4, GGML_TYPE_Q2_K, GGML_TYPE_Q3_K, GGML_TYPE_Q4_0,
    GGML_TYPE_Q4_1, GGML_TYPE_Q4_K, GGML_TYPE_Q5_0, GGML_TYPE_Q5_1, GGML_TYPE_Q5_K,
    GGML_TYPE_Q6_K, GGML_TYPE_Q8_0, GGML_TYPE_Q8_1, GGML_TYPE_Q8_K, GGML_TYPE_TQ1_0,
    GGML_TYPE_TQ2_0,
};

pub fn is_quantized_type(ggml_type: u32) -> bool {
    !matches!(
        ggml_type,
        GGML_TYPE_F32
            | GGML_TYPE_F16
            | GGML_TYPE_I8
            | GGML_TYPE_I16
            | GGML_TYPE_I32
            | GGML_TYPE_I64
            | GGML_TYPE_F64
            | GGML_TYPE_BF16
            | GGML_TYPE_F8_E4M3
    )
}

/// Quantize a row of f32 values into one Q8_0 block (32 elements -> 34 bytes).
/// Finds the absmax, computes scale = absmax/127, quantizes each value to i8.
pub fn quantize_q8_0_block(input: &[f32], out: &mut [u8]) {
    debug_assert!(input.len() >= QK);
    debug_assert!(out.len() >= 34);
    let mut amax = 0.0f32;
    for j in 0..QK {
        let a = input[j].abs();
        if a > amax {
            amax = a;
        }
    }
    let d = amax / 127.0;
    let id = if d != 0.0 { 1.0 / d } else { 0.0 };
    let dh = f32_to_f16(d);
    out[0] = dh as u8;
    out[1] = (dh >> 8) as u8;
    for j in 0..QK {
        let v = (input[j] * id).round();
        let v = v.max(-128.0).min(127.0) as i8;
        out[2 + j] = v as u8;
    }
}

/// Quantize an entire f32 slice to Q8_0 format. Length must be a multiple of QK(32).
pub fn quantize_f32_to_q8_0(input: &[f32]) -> Vec<u8> {
    let n = input.len();
    assert_eq!(
        n % QK,
        0,
        "quantize_f32_to_q8_0: length must be multiple of {}",
        QK
    );
    let nb = n / QK;
    let bs = 34; // block_size for Q8_0
    let mut out = vec![0u8; nb * bs];
    for b in 0..nb {
        quantize_q8_0_block(&input[b * QK..], &mut out[b * bs..]);
    }
    out
}

/// Quantize a row of f32 values into one Q8_1 block (32 elements -> 36 bytes).
pub fn quantize_q8_1_block(input: &[f32], out: &mut [u8]) {
    debug_assert!(input.len() >= QK);
    debug_assert!(out.len() >= block_size(GGML_TYPE_Q8_1));

    let mut amax = 0.0f32;
    for &value in &input[..QK] {
        let abs = value.abs();
        if abs > amax {
            amax = abs;
        }
    }

    let d = amax / 127.0;
    let id = if d != 0.0 { 1.0 / d } else { 0.0 };
    let dh = f32_to_f16(d);
    out[0] = dh as u8;
    out[1] = (dh >> 8) as u8;

    let mut sum = 0i32;
    for j in 0..QK {
        let v = (input[j] * id).round().clamp(-128.0, 127.0) as i8;
        out[4 + j] = v as u8;
        sum += v as i32;
    }

    let sh = f32_to_f16(sum as f32 * d);
    out[2] = sh as u8;
    out[3] = (sh >> 8) as u8;
}

/// Quantize a row of bf16 values into one Q8_1 block (32 elements -> 36 bytes).
pub fn quantize_bf16_q8_1_block(input: &[u16], out: &mut [u8]) {
    debug_assert!(input.len() >= QK);
    debug_assert!(out.len() >= block_size(GGML_TYPE_Q8_1));

    let mut amax = 0.0f32;
    for &word in &input[..QK] {
        let value = bf16_to_f32(word);
        let abs = value.abs();
        if abs > amax {
            amax = abs;
        }
    }

    let d = amax / 127.0;
    let id = if d != 0.0 { 1.0 / d } else { 0.0 };
    let dh = f32_to_f16(d);
    out[0] = dh as u8;
    out[1] = (dh >> 8) as u8;

    let mut sum = 0i32;
    for j in 0..QK {
        let value = bf16_to_f32(input[j]);
        let q = (value * id).round().clamp(-128.0, 127.0) as i8;
        out[4 + j] = q as u8;
        sum += q as i32;
    }

    let sh = f32_to_f16(sum as f32 * d);
    out[2] = sh as u8;
    out[3] = (sh >> 8) as u8;
}

/// Quantize an entire f32 slice to Q8_1 format. Length must be a multiple of QK(32).
pub fn quantize_f32_to_q8_1(input: &[f32]) -> Vec<u8> {
    let n = input.len();
    assert_eq!(
        n % QK,
        0,
        "quantize_f32_to_q8_1: length must be multiple of {}",
        QK
    );
    let nb = n / QK;
    let bs = block_size(GGML_TYPE_Q8_1);
    let mut out = vec![0u8; nb * bs];
    for b in 0..nb {
        quantize_q8_1_block(&input[b * QK..], &mut out[b * bs..]);
    }
    out
}

/// Quantize an entire bf16 slice to Q8_1 format. Length must be a multiple of QK(32).
pub fn quantize_bf16_to_q8_1(input: &[u16]) -> Vec<u8> {
    let n = input.len();
    assert_eq!(
        n % QK,
        0,
        "quantize_bf16_to_q8_1: length must be multiple of {}",
        QK
    );
    let nb = n / QK;
    let bs = block_size(GGML_TYPE_Q8_1);
    let mut out = vec![0u8; nb * bs];
    for b in 0..nb {
        quantize_bf16_q8_1_block(&input[b * QK..], &mut out[b * bs..]);
    }
    out
}

/// Quantize an F16 raw byte slice to Q8_0 format. Length in elements must be multiple of QK(32).
pub fn quantize_f16_to_q8_0(f16_data: &[u8], n_elements: usize) -> Vec<u8> {
    assert_eq!(
        n_elements % QK,
        0,
        "quantize_f16_to_q8_0: length must be multiple of {}",
        QK
    );
    let nb = n_elements / QK;
    let bs = 34;
    let mut out = vec![0u8; nb * bs];
    let mut tmp = [0.0f32; QK];
    for b in 0..nb {
        let base = b * QK * 2;
        for j in 0..QK {
            let off = base + j * 2;
            tmp[j] = f16_to_f32(u16::from_le_bytes([f16_data[off], f16_data[off + 1]]));
        }
        quantize_q8_0_block(&tmp, &mut out[b * bs..]);
    }
    out
}

#[cfg(test)]
mod kquant_tests {
    use super::*;

    #[test]
    fn f32_to_f16_rn_roundtrips_every_finite_f16() {
        for h in 0u16..=0xffff {
            let value = f16_to_f32(h);
            if !value.is_finite() {
                continue;
            }
            assert_eq!(f32_to_f16_rn(value), h, "roundtrip {h:#06x} ({value})");
        }
    }

    #[test]
    fn f32_to_f16_rn_rounds_to_nearest_even() {
        // 1.0 + 2^-11 sits exactly between 1.0 and the next f16; the even
        // mantissa (1.0) wins.  Add one f32 ulp and it must round up.
        assert_eq!(f32_to_f16_rn(1.0 + 2.0f32.powi(-11)), 0x3c00);
        assert_eq!(
            f32_to_f16_rn(f32::from_bits((1.0f32 + 2.0f32.powi(-11)).to_bits() + 1)),
            0x3c01
        );
        // 0.3 rounds up to 0x34cd where the legacy truncation gives 0x34cc.
        assert_eq!(f32_to_f16_rn(0.3), 0x34cd);
        assert_eq!(f32_to_f16(0.3), 0x34cc);
        // Overflow boundary: 65504 is the f16 max, 65519.99 still rounds
        // down to it, the 65520 tie prefers the even infinity.
        assert_eq!(f32_to_f16_rn(65504.0), 0x7bff);
        assert_eq!(f32_to_f16_rn(65519.96), 0x7bff);
        assert_eq!(f32_to_f16_rn(65520.0), 0x7c00);
        assert_eq!(f32_to_f16_rn(f32::INFINITY), 0x7c00);
        assert_eq!(f32_to_f16_rn(-65520.0), 0xfc00);
        // Subnormal boundary: 2^-25 ties down to zero, anything above it
        // rounds to the smallest subnormal, 2^-24 is exact.
        assert_eq!(f32_to_f16_rn(2.0f32.powi(-25)), 0x0000);
        assert_eq!(
            f32_to_f16_rn(f32::from_bits(2.0f32.powi(-25).to_bits() + 1)),
            0x0001
        );
        assert_eq!(f32_to_f16_rn(2.0f32.powi(-24)), 0x0001);
        assert_eq!(f32_to_f16_rn(-2.0f32.powi(-24)), 0x8001);
        assert!(f32_to_f16_rn(f32::NAN) & 0x7c00 == 0x7c00);
        assert!(f32_to_f16_rn(f32::NAN) & 0x03ff != 0);
    }

    #[test]
    fn q4_k_block_dequant_matches_hand_computation() {
        let mut block = vec![0u8; block_size(GGML_TYPE_Q4_K)];
        block[0..2].copy_from_slice(&f32_to_f16(1.0).to_le_bytes()); // d
        block[2..4].copy_from_slice(&f32_to_f16(1.0).to_le_bytes()); // dmin
        // 6-bit packed scales: sc0=2, m0=1; everything else zero.
        block[4] = 2;
        block[8] = 1;
        // First quant byte: low nibble 3 (value index 0), high nibble 5
        // (value index 32).
        block[16] = 0x53;
        let mut out = [0.0f32; QK_K];
        dequantize_q4_k(&block, &mut out);
        // y[0] = d*sc0*3 - dmin*m0 = 2*3 - 1
        assert_eq!(out[0], 5.0);
        // y[32] pairs with (sc1, m1) = (0, 0): 0*5 - 0.
        assert_eq!(out[32], 0.0);
        // Remaining groups have zero scales and zero mins.
        assert!(out[64..].iter().all(|&v| v == 0.0));

        // Row helper agrees via the public gather entry point.
        let gathered =
            get_rows_ggml_bytes_cpu(&block, GGML_TYPE_Q4_K, QK_K, 1, &[0]).unwrap();
        assert_eq!(&gathered[..], &out[..]);
    }

    #[test]
    fn q6_k_block_dequant_matches_hand_computation() {
        let mut block = vec![0u8; block_size(GGML_TYPE_Q6_K)];
        block[208..210].copy_from_slice(&f32_to_f16(0.5).to_le_bytes()); // d (trailing)
        block[0] = 0x21; // ql[0]: low nibble 1 (-> y[0] lane), high nibble 2 (-> y[64] lane)
        block[128] = 0x13; // qh[0]: bits0-1=3 (y[0]), bits4-5=1 (y[64])
        block[192] = 2; // scales[0] (i8) for the y[0..16] lane
        block[196] = 3; // scales[4] (i8) for the y[64..80] lane
        let mut out = [0.0f32; QK_K];
        dequantize_q6_k(&block, &mut out);
        // q1 = (1 | 3<<4) - 32 = 17; y[0] = 0.5 * 2 * 17
        assert_eq!(out[0], 17.0);
        // q3 = (2 | 1<<4) - 32 = -14; y[64] = 0.5 * 3 * -14
        assert_eq!(out[64], -21.0);
        // Lanes with zero scales stay zero.
        assert_eq!(out[32], 0.0);
        assert_eq!(out[96], 0.0);
        let gathered =
            get_rows_ggml_bytes_cpu(&block, GGML_TYPE_Q6_K, QK_K, 1, &[0]).unwrap();
        assert_eq!(&gathered[..], &out[..]);
    }

    #[test]
    fn nvfp4_pairs_row_dequant_and_pack() {
        // Two 16-wide groups (cols = 32).
        let mut weight = vec![0u8; 16];
        weight[0] = 0x21; // cols 0,1: E2M1 idx 1 (0.5), idx 2 (1.0)
        weight[8] = 0x9F; // cols 16,17: idx 0xF (-6.0), idx 0x9 (-0.5)
        let scales = vec![0x38u8, 0x40u8]; // e4m3 1.0, 2.0
        assert_eq!(e4m3_scale_to_f32(0x38), 1.0);
        assert_eq!(e4m3_scale_to_f32(0x40), 2.0);
        let mut out = [0.0f32; 32];
        dequantize_nvfp4_pairs_row(&weight, &scales, 2.0, None, &mut out);
        assert_eq!(out[0], 2.0 * 1.0 * 0.5);
        assert_eq!(out[1], 2.0 * 1.0 * 1.0);
        assert_eq!(out[16], 2.0 * 2.0 * -6.0);
        assert_eq!(out[17], 2.0 * 2.0 * -0.5);
        assert_eq!(out[2], 0.0);

        // AWQ pre-scale folds per input column.
        let mut pre = vec![1.0f32; 32];
        pre[0] = 0.5;
        let mut out_pre = [0.0f32; 32];
        dequantize_nvfp4_pairs_row(&weight, &scales, 2.0, Some(&pre), &mut out_pre);
        assert_eq!(out_pre[0], 0.5);
        assert_eq!(out_pre[1], out[1]);

        // Packed blob: header fields + section sizes.
        let scale_rows = vec![0u8; 4 * 2]; // rows=4, cols=32 -> 2 scale bytes/row
        let weight_rows = vec![0u8; 4 * 16];
        let blob = h3_nvfp4_pairs_pack(4, 32, 1.5, &scale_rows, &weight_rows, None).unwrap();
        assert_eq!(blob.len(), h3_nvfp4_pairs_bytes(4, 32, false).unwrap());
        assert_eq!(&blob[0..4], &H3_NVFP4_PAIRS_MAGIC.to_le_bytes());
        assert_eq!(u32::from_le_bytes(blob[4..8].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(blob[8..12].try_into().unwrap()), 32);
        assert_eq!(u32::from_le_bytes(blob[12..16].try_into().unwrap()), 0);
        assert_eq!(f32::from_le_bytes(blob[16..20].try_into().unwrap()), 1.5);
        // Mismatched section sizes are rejected, not truncated.
        assert!(h3_nvfp4_pairs_pack(4, 32, 1.0, &scale_rows[1..], &weight_rows, None).is_none());
        // cols must be 16-aligned.
        assert!(h3_nvfp4_pairs_bytes(4, 24, false).is_none());
    }

    /// Signed E4M3FN byte goldens for the FLUX combined-FP8 checkpoints.
    /// These anchor the SIGNED decode against the torch float8_e4m3fn
    /// convention; the unsigned NVFP4 scale helpers must never be used for
    /// weight payloads (ue4m3_to_f32 drops the sign and halves the value).
    #[test]
    fn f8_e4m3_signed_decode_goldens() {
        // Zeroes (both signs decode to 0.0).
        assert_eq!(f8_e4m3_to_f32(0x00), 0.0);
        assert_eq!(f8_e4m3_to_f32(0x80), 0.0);
        // Subnormal minimum and first normal.
        assert_eq!(f8_e4m3_to_f32(0x01), 2f32.powi(-9));
        assert_eq!(f8_e4m3_to_f32(0x08), 2f32.powi(-6));
        // Unit values.
        assert_eq!(f8_e4m3_to_f32(0x38), 1.0);
        assert_eq!(f8_e4m3_to_f32(0xB8), -1.0);
        // Extremes (E4M3FN has no infinities; max finite is +-448).
        assert_eq!(f8_e4m3_to_f32(0x7e), 448.0);
        assert_eq!(f8_e4m3_to_f32(0xfe), -448.0);
        // NaN encodings fail closed at the loader; the decode itself is NaN.
        assert!(f8_e4m3_to_f32(0x7f).is_nan());
        assert!(f8_e4m3_to_f32(0xff).is_nan());
        // The sign bit only flips the sign, for every finite magnitude.
        for lo in 0x00..=0x7eu8 {
            assert_eq!(f8_e4m3_to_f32(lo | 0x80), -f8_e4m3_to_f32(lo));
        }
        // Positive finite domain is monotonically non-decreasing and strictly
        // increasing between distinct magnitudes (no encode collisions).
        let mut prev = f8_e4m3_to_f32(0x00);
        for byte in 0x01..=0x7eu8 {
            let value = f8_e4m3_to_f32(byte);
            assert!(
                value > prev,
                "E4M3FN magnitude must strictly increase: byte {byte:#04x} gave {value} after {prev}"
            );
            prev = value;
        }
        // Divergence guard vs the unsigned scale helpers this decode must
        // never be confused with: ue4m3 halves (+1.0 -> 0.5) and drops sign.
        assert_eq!(ue4m3_to_f32(0x38), 0.5);
        assert_eq!(ue4m3_to_f32(0xB8), ue4m3_to_f32(0x38));
    }

    #[test]
    fn f8_e4m3_rows_gather_on_cpu() {
        // 3 rows x 4 cols of raw E4M3FN bytes; gather rows 2 and 0.
        let src = [
            0x38u8, 0xB8, 0x00, 0x01, // row 0: 1, -1, 0, 2^-9
            0x40, 0x48, 0x50, 0x58, // row 1: 2, 4, 8, 16 (not gathered)
            0x7e, 0xfe, 0x08, 0x81, // row 2: 448, -448, 2^-6, -2^-9
        ];
        let out = get_rows_ggml_bytes_cpu(&src, GGML_TYPE_F8_E4M3, 4, 3, &[2, 0]).unwrap();
        assert_eq!(
            out,
            vec![
                448.0,
                -448.0,
                2f32.powi(-6),
                -(2f32.powi(-9)),
                1.0,
                -1.0,
                0.0,
                2f32.powi(-9)
            ]
        );
        // Row length must match n_cols exactly for the 1-byte scalar type.
        assert!(get_rows_ggml_bytes_cpu(&src[..11], GGML_TYPE_F8_E4M3, 4, 3, &[0]).is_none());
    }
}
