// Quantization block formats matching GGML's layout.
// All blocks quantize 32 elements (QK=32).

pub const QK: usize = 32;

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

/// Convert bf16 to f32.
#[inline]
pub fn bf16_to_f32(b: u16) -> f32 {
    f32::from_bits((b as u32) << 16)
}

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
        GGML_TYPE_Q4_0 | GGML_TYPE_Q4_1 | GGML_TYPE_Q5_0 | GGML_TYPE_Q5_1 | GGML_TYPE_Q8_0
        | GGML_TYPE_Q5_K => {
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
            GGML_TYPE_Q4_0 => dequantize_row_blocks(row_src, n_cols, 18, dequantize_q4_0, &mut out),
            GGML_TYPE_Q4_1 => dequantize_row_blocks(row_src, n_cols, 20, dequantize_q4_1, &mut out),
            GGML_TYPE_Q5_0 => dequantize_row_blocks(row_src, n_cols, 22, dequantize_q5_0, &mut out),
            GGML_TYPE_Q5_1 => dequantize_row_blocks(row_src, n_cols, 24, dequantize_q5_1, &mut out),
            GGML_TYPE_Q8_0 => dequantize_row_blocks(row_src, n_cols, 34, dequantize_q8_0, &mut out),
            GGML_TYPE_Q5_K => dequantize_row_q5_k(row_src, n_cols, &mut out),
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
                    d2 * (((ql[l] >> 4) as f32) + if (qh[l] & u2) != 0 { 16.0 } else { 0.0 })
                        - m2,
                );
            }
            ql_offset += 32;
            is += 2;
            u1 <<= 2;
            u2 <<= 2;
        }
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

/// GGML type constants copied from upstream `ggml.h`.
pub const GGML_TYPE_F32: u32 = 0;
pub const GGML_TYPE_F16: u32 = 1;
pub const GGML_TYPE_Q4_0: u32 = 2;
pub const GGML_TYPE_Q4_1: u32 = 3;
pub const GGML_TYPE_Q5_0: u32 = 6;
pub const GGML_TYPE_Q5_1: u32 = 7;
pub const GGML_TYPE_Q8_0: u32 = 8;
pub const GGML_TYPE_Q8_1: u32 = 9;
pub const GGML_TYPE_Q2_K: u32 = 10;
pub const GGML_TYPE_Q3_K: u32 = 11;
pub const GGML_TYPE_Q4_K: u32 = 12;
pub const GGML_TYPE_Q5_K: u32 = 13;
pub const GGML_TYPE_Q6_K: u32 = 14;
pub const GGML_TYPE_Q8_K: u32 = 15;
pub const GGML_TYPE_IQ2_XXS: u32 = 16;
pub const GGML_TYPE_IQ2_XS: u32 = 17;
pub const GGML_TYPE_IQ3_XXS: u32 = 18;
pub const GGML_TYPE_IQ1_S: u32 = 19;
pub const GGML_TYPE_IQ4_NL: u32 = 20;
pub const GGML_TYPE_IQ3_S: u32 = 21;
pub const GGML_TYPE_IQ2_S: u32 = 22;
pub const GGML_TYPE_IQ4_XS: u32 = 23;
pub const GGML_TYPE_I8: u32 = 24;
pub const GGML_TYPE_I16: u32 = 25;
pub const GGML_TYPE_I32: u32 = 26;
pub const GGML_TYPE_I64: u32 = 27;
pub const GGML_TYPE_F64: u32 = 28;
pub const GGML_TYPE_IQ1_M: u32 = 29;
pub const GGML_TYPE_BF16: u32 = 30;
pub const GGML_TYPE_TQ1_0: u32 = 34;
pub const GGML_TYPE_TQ2_0: u32 = 35;
pub const GGML_TYPE_MXFP4: u32 = 39;
pub const GGML_TYPE_NVFP4: u32 = 40;
pub const GGML_TYPE_COUNT: u32 = 41;

pub fn ggml_type_name(ggml_type: u32) -> &'static str {
    match ggml_type {
        GGML_TYPE_F32 => "f32",
        GGML_TYPE_F16 => "f16",
        GGML_TYPE_Q4_0 => "q4_0",
        GGML_TYPE_Q4_1 => "q4_1",
        GGML_TYPE_Q5_0 => "q5_0",
        GGML_TYPE_Q5_1 => "q5_1",
        GGML_TYPE_Q8_0 => "q8_0",
        GGML_TYPE_Q8_1 => "q8_1",
        GGML_TYPE_Q2_K => "q2_K",
        GGML_TYPE_Q3_K => "q3_K",
        GGML_TYPE_Q4_K => "q4_K",
        GGML_TYPE_Q5_K => "q5_K",
        GGML_TYPE_Q6_K => "q6_K",
        GGML_TYPE_Q8_K => "q8_K",
        GGML_TYPE_IQ2_XXS => "iq2_xxs",
        GGML_TYPE_IQ2_XS => "iq2_xs",
        GGML_TYPE_IQ3_XXS => "iq3_xxs",
        GGML_TYPE_IQ1_S => "iq1_s",
        GGML_TYPE_IQ4_NL => "iq4_nl",
        GGML_TYPE_IQ3_S => "iq3_s",
        GGML_TYPE_IQ2_S => "iq2_s",
        GGML_TYPE_IQ4_XS => "iq4_xs",
        GGML_TYPE_I8 => "i8",
        GGML_TYPE_I16 => "i16",
        GGML_TYPE_I32 => "i32",
        GGML_TYPE_I64 => "i64",
        GGML_TYPE_F64 => "f64",
        GGML_TYPE_IQ1_M => "iq1_m",
        GGML_TYPE_BF16 => "bf16",
        GGML_TYPE_TQ1_0 => "tq1_0",
        GGML_TYPE_TQ2_0 => "tq2_0",
        GGML_TYPE_MXFP4 => "mxfp4",
        GGML_TYPE_NVFP4 => "nvfp4",
        _ => "unknown",
    }
}

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
    )
}

/// Type/block size in bytes for one ggml storage block.
pub fn block_size(ggml_type: u32) -> usize {
    match ggml_type {
        GGML_TYPE_F32 => 4,
        GGML_TYPE_F16 => 2,
        GGML_TYPE_Q4_0 => 18,
        GGML_TYPE_Q4_1 => 20,
        GGML_TYPE_Q5_0 => 22,
        GGML_TYPE_Q5_1 => 24,
        GGML_TYPE_Q8_0 => 34,
        GGML_TYPE_Q8_1 => 36,
        GGML_TYPE_Q2_K => 84,
        GGML_TYPE_Q3_K => 110,
        GGML_TYPE_Q4_K => 144,
        GGML_TYPE_Q5_K => 176,
        GGML_TYPE_Q6_K => 210,
        GGML_TYPE_Q8_K => 292,
        GGML_TYPE_IQ2_XXS => 66,
        GGML_TYPE_IQ2_XS => 74,
        GGML_TYPE_IQ3_XXS => 98,
        GGML_TYPE_IQ1_S => 50,
        GGML_TYPE_IQ4_NL => 18,
        GGML_TYPE_IQ3_S => 110,
        GGML_TYPE_IQ2_S => 82,
        GGML_TYPE_IQ4_XS => 136,
        GGML_TYPE_I8 => 1,
        GGML_TYPE_I16 => 2,
        GGML_TYPE_I32 => 4,
        GGML_TYPE_I64 => 8,
        GGML_TYPE_F64 => 8,
        GGML_TYPE_IQ1_M => 56,
        GGML_TYPE_BF16 => 2,
        GGML_TYPE_TQ1_0 => 54,
        GGML_TYPE_TQ2_0 => 66,
        GGML_TYPE_MXFP4 => 17,
        GGML_TYPE_NVFP4 => 36,
        _ => panic!("unsupported ggml type {}", ggml_type),
    }
}

/// Number of dequantized elements represented by one storage block.
pub fn block_elements(ggml_type: u32) -> usize {
    match ggml_type {
        GGML_TYPE_F32 | GGML_TYPE_F16 | GGML_TYPE_I8 | GGML_TYPE_I16 | GGML_TYPE_I32
        | GGML_TYPE_I64 | GGML_TYPE_F64 | GGML_TYPE_BF16 => 1,
        GGML_TYPE_Q4_0 | GGML_TYPE_Q4_1 | GGML_TYPE_Q5_0 | GGML_TYPE_Q5_1 | GGML_TYPE_Q8_0
        | GGML_TYPE_Q8_1 | GGML_TYPE_MXFP4 | GGML_TYPE_IQ4_NL => QK,
        GGML_TYPE_NVFP4 => QK_NVFP4,
        GGML_TYPE_Q2_K | GGML_TYPE_Q3_K | GGML_TYPE_Q4_K | GGML_TYPE_Q5_K | GGML_TYPE_Q6_K
        | GGML_TYPE_Q8_K | GGML_TYPE_IQ2_XXS | GGML_TYPE_IQ2_XS | GGML_TYPE_IQ3_XXS
        | GGML_TYPE_IQ1_S | GGML_TYPE_IQ3_S | GGML_TYPE_IQ2_S | GGML_TYPE_IQ4_XS
        | GGML_TYPE_IQ1_M | GGML_TYPE_TQ1_0 | GGML_TYPE_TQ2_0 => QK_K,
        _ => panic!("unsupported ggml type {}", ggml_type),
    }
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
