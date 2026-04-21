// Port of libdeflate's adler32.c with optional aarch64 NEON intrinsics
// Original: Copyright 2016 Eric Biggers, MIT license

const DIVISOR: u32 = 65521;
const MAX_CHUNK_LEN: usize = 5552;

/// Compute Adler-32 checksum.
pub fn adler32(init: u32, data: &[u8]) -> u32 {
    #[cfg(target_arch = "aarch64")]
    {
        if is_aarch64_neon_available() {
            return adler32_neon(init, data);
        }
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_sse2_available() {
            return unsafe { adler32_sse2(init, data) };
        }
    }
    adler32_generic(init, data)
}

/// Rolling Adler-32 state.
pub struct Adler32 {
    val: u32,
}

impl Default for Adler32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Adler32 {
    pub const fn new() -> Self {
        Adler32 { val: 1 }
    }

    pub const fn with_initial(val: u32) -> Self {
        Adler32 { val }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.val = adler32(self.val, data);
    }

    pub const fn sum(&self) -> u32 {
        self.val
    }
}

// --- Generic (portable) implementation ---
// 4-bytes-at-a-time strategy from libdeflate for better ILP

fn adler32_generic(adler: u32, data: &[u8]) -> u32 {
    let mut s1 = adler & 0xFFFF;
    let mut s2 = adler >> 16;
    let mut p = data;

    while !p.is_empty() {
        let n = p.len().min(MAX_CHUNK_LEN & !3);
        let (chunk, rest) = p.split_at(n);
        p = rest;

        adler32_chunk(&mut s1, &mut s2, chunk);
    }

    (s2 << 16) | s1
}

#[inline(always)]
fn adler32_chunk(s1: &mut u32, s2: &mut u32, data: &[u8]) {
    let mut p = data;

    if p.len() >= 4 {
        let mut s1_sum: u32 = 0;
        let mut byte_0_sum: u32 = 0;
        let mut byte_1_sum: u32 = 0;
        let mut byte_2_sum: u32 = 0;
        let mut byte_3_sum: u32 = 0;

        while p.len() >= 4 {
            s1_sum += *s1;
            *s1 += p[0] as u32 + p[1] as u32 + p[2] as u32 + p[3] as u32;
            byte_0_sum += p[0] as u32;
            byte_1_sum += p[1] as u32;
            byte_2_sum += p[2] as u32;
            byte_3_sum += p[3] as u32;
            p = &p[4..];
        }
        *s2 += 4 * (s1_sum + byte_0_sum) + 3 * byte_1_sum + 2 * byte_2_sum + byte_3_sum;
    }

    for &b in p {
        *s1 += b as u32;
        *s2 += *s1;
    }

    *s1 %= DIVISOR;
    *s2 %= DIVISOR;
}

// --- aarch64 NEON implementation ---

#[cfg(target_arch = "aarch64")]
fn is_aarch64_neon_available() -> bool {
    // NEON is always available on aarch64
    true
}

#[cfg(target_arch = "aarch64")]
fn adler32_neon(adler: u32, data: &[u8]) -> u32 {
    use std::arch::aarch64::*;

    let mut s1 = adler & 0xFFFF;
    let mut s2 = adler >> 16;
    let mut p = data;

    // Align if large
    if p.len() > 32768 && (p.as_ptr() as usize & 15) != 0 {
        while !p.is_empty() && (p.as_ptr() as usize & 15) != 0 {
            s1 += p[0] as u32;
            s2 += s1;
            p = &p[1..];
        }
        s1 %= DIVISOR;
        s2 %= DIVISOR;
    }

    static MULTS: [u16; 64] = [
        64, 63, 62, 61, 60, 59, 58, 57, 56, 55, 54, 53, 52, 51, 50, 49, 48, 47, 46, 45, 44, 43, 42,
        41, 40, 39, 38, 37, 36, 35, 34, 33, 32, 31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19,
        18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1,
    ];

    while !p.is_empty() {
        let n = p.len().min(MAX_CHUNK_LEN & !63);
        let (chunk, rest) = p.split_at(n);
        p = rest;

        if chunk.len() >= 64 {
            unsafe {
                let mults_a = vld1q_u16(MULTS.as_ptr().add(0));
                let mults_b = vld1q_u16(MULTS.as_ptr().add(8));
                let mults_c = vld1q_u16(MULTS.as_ptr().add(16));
                let mults_d = vld1q_u16(MULTS.as_ptr().add(24));
                let mults_e = vld1q_u16(MULTS.as_ptr().add(32));
                let mults_f = vld1q_u16(MULTS.as_ptr().add(40));
                let mults_g = vld1q_u16(MULTS.as_ptr().add(48));
                let mults_h = vld1q_u16(MULTS.as_ptr().add(56));

                let mut v_s1 = vdupq_n_u32(0);
                let mut v_s2 = vdupq_n_u32(0);
                let mut v_byte_sums_a = vdupq_n_u16(0);
                let mut v_byte_sums_b = vdupq_n_u16(0);
                let mut v_byte_sums_c = vdupq_n_u16(0);
                let mut v_byte_sums_d = vdupq_n_u16(0);
                let mut v_byte_sums_e = vdupq_n_u16(0);
                let mut v_byte_sums_f = vdupq_n_u16(0);
                let mut v_byte_sums_g = vdupq_n_u16(0);
                let mut v_byte_sums_h = vdupq_n_u16(0);

                let vec_len = chunk.len() & !63;
                s2 += s1 * vec_len as u32;

                let mut cp = chunk.as_ptr();
                let mut remaining = vec_len;

                while remaining >= 64 {
                    let data_a = vld1q_u8(cp);
                    let data_b = vld1q_u8(cp.add(16));
                    let data_c = vld1q_u8(cp.add(32));
                    let data_d = vld1q_u8(cp.add(48));

                    v_s2 = vaddq_u32(v_s2, v_s1);

                    let mut tmp = vpaddlq_u8(data_a);
                    v_byte_sums_a = vaddw_u8(v_byte_sums_a, vget_low_u8(data_a));
                    v_byte_sums_b = vaddw_u8(v_byte_sums_b, vget_high_u8(data_a));

                    tmp = vpadalq_u8(tmp, data_b);
                    v_byte_sums_c = vaddw_u8(v_byte_sums_c, vget_low_u8(data_b));
                    v_byte_sums_d = vaddw_u8(v_byte_sums_d, vget_high_u8(data_b));

                    tmp = vpadalq_u8(tmp, data_c);
                    v_byte_sums_e = vaddw_u8(v_byte_sums_e, vget_low_u8(data_c));
                    v_byte_sums_f = vaddw_u8(v_byte_sums_f, vget_high_u8(data_c));

                    tmp = vpadalq_u8(tmp, data_d);
                    v_byte_sums_g = vaddw_u8(v_byte_sums_g, vget_low_u8(data_d));
                    v_byte_sums_h = vaddw_u8(v_byte_sums_h, vget_high_u8(data_d));

                    v_s1 = vpadalq_u16(v_s1, tmp);

                    cp = cp.add(64);
                    remaining -= 64;
                }

                // s2 = 64*s2 + weighted byte sums
                v_s2 = vqshlq_n_u32(v_s2, 6);
                v_s2 = vmlal_u16(v_s2, vget_low_u16(v_byte_sums_a), vget_low_u16(mults_a));
                v_s2 = vmlal_high_u16(v_s2, v_byte_sums_a, mults_a);
                v_s2 = vmlal_u16(v_s2, vget_low_u16(v_byte_sums_b), vget_low_u16(mults_b));
                v_s2 = vmlal_high_u16(v_s2, v_byte_sums_b, mults_b);
                v_s2 = vmlal_u16(v_s2, vget_low_u16(v_byte_sums_c), vget_low_u16(mults_c));
                v_s2 = vmlal_high_u16(v_s2, v_byte_sums_c, mults_c);
                v_s2 = vmlal_u16(v_s2, vget_low_u16(v_byte_sums_d), vget_low_u16(mults_d));
                v_s2 = vmlal_high_u16(v_s2, v_byte_sums_d, mults_d);
                v_s2 = vmlal_u16(v_s2, vget_low_u16(v_byte_sums_e), vget_low_u16(mults_e));
                v_s2 = vmlal_high_u16(v_s2, v_byte_sums_e, mults_e);
                v_s2 = vmlal_u16(v_s2, vget_low_u16(v_byte_sums_f), vget_low_u16(mults_f));
                v_s2 = vmlal_high_u16(v_s2, v_byte_sums_f, mults_f);
                v_s2 = vmlal_u16(v_s2, vget_low_u16(v_byte_sums_g), vget_low_u16(mults_g));
                v_s2 = vmlal_high_u16(v_s2, v_byte_sums_g, mults_g);
                v_s2 = vmlal_u16(v_s2, vget_low_u16(v_byte_sums_h), vget_low_u16(mults_h));
                v_s2 = vmlal_high_u16(v_s2, v_byte_sums_h, mults_h);

                s1 += vaddvq_u32(v_s1);
                s2 += vaddvq_u32(v_s2);
            }

            // Scalar tail
            let tail = &chunk[chunk.len() & !63..];
            adler32_chunk(&mut s1, &mut s2, tail);
        } else {
            adler32_chunk(&mut s1, &mut s2, chunk);
        }
    }

    (s2 << 16) | s1
}

// --- x86/x86_64 SSE2 implementation ---
// Port of libdeflate's x86 adler32_template.h (USE_VNNI=0, VL=16)
// Uses psadbw for horizontal byte sums and punpck+pmaddwd for weighted sums.

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn is_x86_sse2_available() -> bool {
    // SSE2 is always available on x86_64; on x86 we check at runtime
    #[cfg(target_arch = "x86_64")]
    {
        true
    }
    #[cfg(target_arch = "x86")]
    {
        is_x86_feature_detected!("sse2")
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "sse2")]
unsafe fn adler32_sse2(adler: u32, data: &[u8]) -> u32 {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    // VL=16, so we process 2*VL=32 bytes per inner iteration.
    // Multiplier tables for weighted byte sums: [2*VL, 2*VL-1, ..., 1]
    // Split into 4 groups matching punpcklbw/punpckhbw lane ordering.
    #[repr(align(16))]
    struct Aligned128([i16; 8]);

    static MULTS_A: Aligned128 = Aligned128([32, 31, 30, 29, 28, 27, 26, 25]);
    static MULTS_B: Aligned128 = Aligned128([24, 23, 22, 21, 20, 19, 18, 17]);
    static MULTS_C: Aligned128 = Aligned128([16, 15, 14, 13, 12, 11, 10, 9]);
    static MULTS_D: Aligned128 = Aligned128([8, 7, 6, 5, 4, 3, 2, 1]);

    let mults_a = _mm_load_si128(MULTS_A.0.as_ptr() as *const __m128i);
    let mults_b = _mm_load_si128(MULTS_B.0.as_ptr() as *const __m128i);
    let mults_c = _mm_load_si128(MULTS_C.0.as_ptr() as *const __m128i);
    let mults_d = _mm_load_si128(MULTS_D.0.as_ptr() as *const __m128i);
    let zeroes = _mm_setzero_si128();

    let mut s1 = adler & 0xFFFF;
    let mut s2 = adler >> 16;
    let mut p = data;

    // For large data, align pointer
    if p.len() > 65536 && (p.as_ptr() as usize & 15) != 0 {
        while !p.is_empty() && (p.as_ptr() as usize & 15) != 0 {
            s1 += p[0] as u32;
            s2 += s1;
            p = &p[1..];
        }
        s1 %= DIVISOR;
        s2 %= DIVISOR;
    }

    // INT16_MAX / UINT8_MAX = 128, so max chunk is 2*16*128 = 4096 bytes
    // (limited by 16-bit byte_sums counters used with pmaddwd which is signed)
    const SSE2_MAX_CHUNK: usize = 4096;

    while !p.is_empty() {
        let n = p.len().min(SSE2_MAX_CHUNK & !(32 - 1));
        let (chunk, rest) = p.split_at(n);
        p = rest;

        if chunk.len() >= 32 {
            let mut v_s1 = zeroes;
            let mut v_s1_sums = zeroes;
            let mut v_byte_sums_a = zeroes;
            let mut v_byte_sums_b = zeroes;
            let mut v_byte_sums_c = zeroes;
            let mut v_byte_sums_d = zeroes;

            let vec_len = chunk.len() & !31;
            s2 += s1 * vec_len as u32;

            let mut cp = chunk.as_ptr();
            let mut remaining = vec_len;

            while remaining >= 32 {
                let data_a = _mm_loadu_si128(cp as *const __m128i);
                let data_b = _mm_loadu_si128(cp.add(16) as *const __m128i);

                // Accumulate s1_sums for later s2 calculation
                v_s1_sums = _mm_add_epi32(v_s1_sums, v_s1);

                // Unpack bytes to 16-bit and accumulate per-position sums
                v_byte_sums_a = _mm_add_epi16(v_byte_sums_a, _mm_unpacklo_epi8(data_a, zeroes));
                v_byte_sums_b = _mm_add_epi16(v_byte_sums_b, _mm_unpackhi_epi8(data_a, zeroes));
                v_byte_sums_c = _mm_add_epi16(v_byte_sums_c, _mm_unpacklo_epi8(data_b, zeroes));
                v_byte_sums_d = _mm_add_epi16(v_byte_sums_d, _mm_unpackhi_epi8(data_b, zeroes));

                // Sum all bytes for s1 using SAD against zero
                v_s1 = _mm_add_epi32(
                    v_s1,
                    _mm_add_epi32(_mm_sad_epu8(data_a, zeroes), _mm_sad_epu8(data_b, zeroes)),
                );

                cp = cp.add(32);
                remaining -= 32;
            }

            // v_s2 = 32*v_s1_sums + [32,31,...,1] dot v_byte_sums
            let v_s2 = _mm_add_epi32(
                _mm_add_epi32(
                    _mm_slli_epi32(v_s1_sums, 5), // 32 * v_s1_sums
                    _mm_add_epi32(
                        _mm_madd_epi16(v_byte_sums_a, mults_a),
                        _mm_madd_epi16(v_byte_sums_b, mults_b),
                    ),
                ),
                _mm_add_epi32(
                    _mm_madd_epi16(v_byte_sums_c, mults_c),
                    _mm_madd_epi16(v_byte_sums_d, mults_d),
                ),
            );

            // Horizontal reduce v_s1 (128 -> 32 bits)
            // v_s1 has values in elements 0 and 2 (from psadbw), elements 1 and 3 are zero
            let v_s1_hi = _mm_shuffle_epi32(v_s1, 0x02); // element 2 -> element 0
            let v_s1_sum = _mm_add_epi32(v_s1, v_s1_hi);
            s1 += _mm_cvtsi128_si32(v_s1_sum) as u32;

            // Horizontal reduce v_s2 (128 -> 32 bits)
            let v_s2_1 = _mm_shuffle_epi32(v_s2, 0x31); // elements 1,3
            let v_s2_sum = _mm_add_epi32(v_s2, v_s2_1);
            let v_s2_2 = _mm_shuffle_epi32(v_s2_sum, 0x02);
            let v_s2_final = _mm_add_epi32(v_s2_sum, v_s2_2);
            s2 += _mm_cvtsi128_si32(v_s2_final) as u32;
        }

        // Scalar tail
        let tail = &chunk[chunk.len() & !31..];
        adler32_chunk(&mut s1, &mut s2, tail);
    }

    (s2 << 16) | s1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adler32_empty() {
        assert_eq!(adler32(1, &[]), 1);
    }

    #[test]
    fn test_adler32_hello() {
        // Known value: adler32("Hello") = 0x058c01f5
        let val = adler32(1, b"Hello");
        assert_eq!(val, 0x058c01f5);
    }

    #[test]
    fn test_adler32_rolling() {
        let data = b"Hello, World!";
        let one_shot = adler32(1, data);

        let mut rolling = Adler32::new();
        rolling.update(&data[..5]);
        rolling.update(&data[5..]);
        assert_eq!(rolling.sum(), one_shot);
    }

    #[test]
    fn test_adler32_various_sizes() {
        // Test accelerated path against generic for various sizes
        let data: Vec<u8> = (0..=255).cycle().take(8192).collect();
        for size in [
            1, 2, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 512, 1024, 4096, 5552,
            8192,
        ] {
            let slice = &data[..size];
            let expected = adler32_generic(1, slice);
            let got = adler32(1, slice);
            assert_eq!(got, expected, "mismatch at size {}", size);
        }
    }

    #[test]
    fn test_adler32_incremental_vs_oneshot() {
        // Test rolling across chunk boundaries
        let data: Vec<u8> = (0..=255).cycle().take(12000).collect();
        let one_shot = adler32(1, &data);

        let mut rolling = Adler32::new();
        for chunk in data.chunks(137) {
            rolling.update(chunk);
        }
        assert_eq!(rolling.sum(), one_shot);
    }
}
