// Differential reference: rustc --edition 2021 --test -O simd_intrinsics.rs
// Run with --nocapture --test-threads=1 and compare the SIMD lines with makepad test.
// Every bridge wrapper is exercised, including its separately compiled body, across
// lane extrema, saturation boundaries, shuffle sign bits, NaNs and signed zero.
#![allow(unused_unsafe, non_snake_case)]

fn mix<T>(h: &mut u64, v: &T) {
    let bytes = unsafe {
        core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>())
    };
    for &b in bytes {
        *h = (*h ^ b as u64).wrapping_mul(0x100000001b3);
    }
}

fn input<T: Copy>(n: usize) -> T {
    let edges = [
        0u64,
        u64::MAX,
        0x8080808080808080,
        0x7f7f7f7f7f7f7f7f,
        0x8000800080008000,
        0x7fff7fff7fff7fff,
        0x8000000080000000,
        0x7fffffff7fffffff,
        0x8000000000000000,
        0x7fffffffffffffff,
        0x00ff0100007f0080,
        0xffff00010000ffff,
        0x0000ffff00010000,
        0x000102030e0f1011,
        0x7f80ff1020304050,
        0x0123456789abcdef,
    ];
    let mut words = [0u64; 4];
    for i in 0..4 {
        words[i] = edges[(n + i * (n / 16)) % 16];
    }
    unsafe { core::ptr::read_unaligned(words.as_ptr() as *const T) }
}

fn float_input<T: Copy>(n: usize) -> T {
    // One quiet NaN payload, to avoid unspecified choices between two payloads
    // in commutative arithmetic; min/max still verify NaN and signed-zero bits.
    let bits = [
        0u32, 0x80000000, 0x3f800000, 0xbf800000, 0x7f800000, 0xff800000, 0x7fc00000, 1,
        0x7f7fffff, 0x00800000, 0x40800000, 0xc0800000,
    ];
    let mut words = [0u32; 4];
    for i in 0..4 {
        words[i] = bits[(n + i) % 12];
    }
    unsafe { core::ptr::read_unaligned(words.as_ptr() as *const T) }
}

// Exercise both NaN operands and signaling/quiet priority as well as signed zeros.
fn minmax_input<T: Copy>(n: usize) -> T {
    let bits = [
        0u32, 0x80000000, 0x3f800000, 0xbf800000, 0x7f800000, 0xff800000, 0x7fc12345, 0xffc54321,
        0x7f800001, 0xff800123, 1, 0x80000001, 0x7f7fffff, 0xff7fffff, 0x00800000, 0x80800000,
    ];
    let mut words = [0u32; 4];
    for i in 0..4 {
        words[i] = bits[(n + i) % 16];
    }
    unsafe { core::ptr::read_unaligned(words.as_ptr() as *const T) }
}

#[cfg(target_arch = "x86_64")]
mod x86_64 {
    use super::*;
    use core::arch::x86_64::*;

    #[inline(never)]
    unsafe fn probe_mm_abs_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_abs_epi16(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_add_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_add_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_add_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_add_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_add_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_add_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_and_si128(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_and_si128(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_andnot_si128(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_andnot_si128(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_avg_epu8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_avg_epu8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_blendv_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let mask: __m128i = input(n + 10);
        let out = _mm_blendv_epi8(a, b, mask);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpeq_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_cmpeq_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpeq_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_cmpeq_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpgt_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_cmpgt_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_load_si128(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let mem_addr = buf.0.as_mut_ptr().add(16) as *const __m128i;
        let out = _mm_load_si128(mem_addr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_loadu_si128(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let mem_addr = buf.0.as_mut_ptr().add(17) as *const __m128i;
        let out = _mm_loadu_si128(mem_addr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_madd_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_madd_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_maddubs_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_maddubs_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_min_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_min_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_movemask_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_movemask_epi8(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_or_si128(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_or_si128(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_packus_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_packus_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_sad_epu8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_sad_epu8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_set1_epi16(n: usize, h: &mut u64) {
        let a: i16 = input(n + 0);
        let out = _mm_set1_epi16(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_set1_epi8(n: usize, h: &mut u64) {
        let a: i8 = input(n + 0);
        let out = _mm_set1_epi8(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_setzero_si128(_n: usize, h: &mut u64) {
        let out = _mm_setzero_si128();
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi32_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_shuffle_epi32::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_shuffle_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_shuffle_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi32_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi32::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_si128_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_si128::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_store_si128(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let mem_addr = buf.0.as_mut_ptr().add(16) as *mut __m128i;
        let a: __m128i = input(n + 5);
        _mm_store_si128(mem_addr, a);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probe_mm_storeu_si128(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let mem_addr = buf.0.as_mut_ptr().add(17) as *mut __m128i;
        let a: __m128i = input(n + 5);
        _mm_storeu_si128(mem_addr, a);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probe_mm_sub_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_sub_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_sub_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_sub_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_unpackhi_epi64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_unpackhi_epi64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_unpackhi_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_unpackhi_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_unpacklo_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_unpacklo_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_xor_si128(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_xor_si128(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_add_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_add_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_and_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_and_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_andnot_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_andnot_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpeq_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_cmpeq_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpeq_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_cmpeq_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpge_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_cmpge_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpgt_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_cmpgt_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpgt_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_cmpgt_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpgt_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_cmpgt_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmple_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_cmple_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmplt_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_cmplt_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_div_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_div_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_loadu_ps(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let mem_addr = buf.0.as_mut_ptr().add(17) as *const f32;
        let out = _mm_loadu_ps(mem_addr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_max_ps(n: usize, h: &mut u64) {
        let a: __m128 = minmax_input(n % 16);
        let b: __m128 = minmax_input(n / 16);
        let out = _mm_max_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_min_ps(n: usize, h: &mut u64) {
        let a: __m128 = minmax_input(n % 16);
        let b: __m128 = minmax_input(n / 16);
        let out = _mm_min_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_mul_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_mul_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_or_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_or_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_set1_ps(n: usize, h: &mut u64) {
        let a: f32 = float_input(n + 0);
        let out = _mm_set1_ps(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi16_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi16::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_slli_epi64_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_slli_epi64::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi32_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi32::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi16_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi16::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi32_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi32::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srli_epi64_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srli_epi64::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_sqrt_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let out = _mm_sqrt_ps(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_storeu_ps(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let mem_addr = buf.0.as_mut_ptr().add(17) as *mut f32;
        let a: __m128 = float_input(n + 5);
        _mm_storeu_ps(mem_addr, a);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probe_mm_sub_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_sub_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_unpackhi_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_unpackhi_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_unpackhi_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_unpackhi_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_unpacklo_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_unpacklo_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_unpacklo_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_unpacklo_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_unpacklo_epi64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_unpacklo_epi64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_xor_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_xor_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_add_epi64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_add_epi64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_sub_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_sub_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_sub_epi64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_sub_epi64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_adds_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_adds_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_adds_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_adds_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_adds_epu8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_adds_epu8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_adds_epu16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_adds_epu16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_subs_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_subs_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_subs_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_subs_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_subs_epu8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_subs_epu8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_subs_epu16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_subs_epu16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_min_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_min_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_min_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_min_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_min_epu8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_min_epu8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_min_epu16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_min_epu16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_min_epu32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_min_epu32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_max_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_max_epi8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_max_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_max_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_max_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_max_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_max_epu8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_max_epu8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_max_epu16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_max_epu16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_max_epu32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_max_epu32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_avg_epu16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_avg_epu16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_abs_epi8(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_abs_epi8(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_abs_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_abs_epi32(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_set1_epi32(n: usize, h: &mut u64) {
        let a: i32 = input(n + 0);
        let out = _mm_set1_epi32(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_set1_epi64x(n: usize, h: &mut u64) {
        let a: i64 = input(n + 0);
        let out = _mm_set1_epi64x(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_packs_epi16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_packs_epi16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_packs_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_packs_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_packus_epi32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let b: __m128i = input(n + 5);
        let out = _mm_packus_epi32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi16_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi16::<255>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_cmpneq_ps(n: usize, h: &mut u64) {
        let a: __m128 = float_input(n + 0);
        let b: __m128 = float_input(n + 5);
        let out = _mm_cmpneq_ps(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_0(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<0>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_1(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_7(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_15(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_16(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_31(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_32(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_63(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_64(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probe_mm_srai_epi64_255(n: usize, h: &mut u64) {
        let a: __m128i = input(n + 0);
        let out = _mm_srai_epi64::<255>(a);
        mix(h, &out);
    }

    #[test]
    fn simd_intrinsics() {
        // Warm every probe before the measured pass so asynchronous O3 builds
        // can finish while the other probes run. The printed hashes stay stable.
        simd_hashes(false);
        simd_hashes(true);
    }

    #[inline(never)]
    fn simd_hashes(report: bool) {
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_abs_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_abs_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_add_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_add_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_add_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_add_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_add_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_add_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_and_si128(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_and_si128 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_andnot_si128(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_andnot_si128 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_avg_epu8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_avg_epu8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_blendv_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_blendv_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpeq_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpeq_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpeq_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpeq_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpgt_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpgt_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_load_si128(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_load_si128 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_loadu_si128(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_loadu_si128 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_madd_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_madd_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_maddubs_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_maddubs_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_min_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_min_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_movemask_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_movemask_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_or_si128(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_or_si128 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_packus_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_packus_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_sad_epu8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_sad_epu8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_set1_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_set1_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_set1_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_set1_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_setzero_si128(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_setzero_si128 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi32_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi32_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_shuffle_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_shuffle_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi32_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi32_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_si128_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_si128_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_store_si128(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_store_si128 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_storeu_si128(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_storeu_si128 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_sub_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_sub_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_sub_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_sub_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_unpackhi_epi64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_unpackhi_epi64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_unpackhi_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_unpackhi_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_unpacklo_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_unpacklo_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_xor_si128(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_xor_si128 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_add_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_add_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_and_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_and_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_andnot_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_andnot_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpeq_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpeq_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpeq_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpeq_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpge_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpge_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpgt_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpgt_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpgt_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpgt_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpgt_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpgt_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmple_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmple_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmplt_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmplt_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_div_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_div_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_loadu_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_loadu_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_max_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_max_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_min_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_min_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_mul_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_mul_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_or_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_or_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_set1_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_set1_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi16_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi16_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_slli_epi64_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_slli_epi64_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi32_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi32_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi16_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi16_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi32_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi32_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srli_epi64_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srli_epi64_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_sqrt_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_sqrt_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_storeu_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_storeu_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_sub_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_sub_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_unpackhi_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_unpackhi_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_unpackhi_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_unpackhi_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_unpacklo_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_unpacklo_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_unpacklo_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_unpacklo_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_unpacklo_epi64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_unpacklo_epi64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_xor_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_xor_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_add_epi64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_add_epi64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_sub_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_sub_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_sub_epi64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_sub_epi64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_adds_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_adds_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_adds_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_adds_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_adds_epu8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_adds_epu8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_adds_epu16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_adds_epu16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_subs_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_subs_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_subs_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_subs_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_subs_epu8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_subs_epu8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_subs_epu16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_subs_epu16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_min_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_min_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_min_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_min_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_min_epu8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_min_epu8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_min_epu16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_min_epu16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_min_epu32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_min_epu32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_max_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_max_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_max_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_max_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_max_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_max_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_max_epu8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_max_epu8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_max_epu16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_max_epu16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_max_epu32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_max_epu32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_avg_epu16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_avg_epu16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_abs_epi8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_abs_epi8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_abs_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_abs_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_set1_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_set1_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_set1_epi64x(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_set1_epi64x {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_packs_epi16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_packs_epi16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_packs_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_packs_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_packus_epi32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_packus_epi32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi16_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi16_255 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_cmpneq_ps(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_cmpneq_ps {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_0(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_0 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probe_mm_srai_epi64_255(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD _mm_srai_epi64_255 {:016x}", h);
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use super::*;
    use core::arch::aarch64::*;

    #[inline(never)]
    unsafe fn probevaddq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vaddq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevaddq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vaddq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevaddq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vaddq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_s16(n: usize, h: &mut u64) {
        let value: i16 = input(n + 0);
        let out = vdupq_n_s16(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_s32(n: usize, h: &mut u64) {
        let value: i32 = input(n + 0);
        let out = vdupq_n_s32(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_u16(n: usize, h: &mut u64) {
        let value: u16 = input(n + 0);
        let out = vdupq_n_u16(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_u32(n: usize, h: &mut u64) {
        let value: u32 = input(n + 0);
        let out = vdupq_n_u32(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_u8(n: usize, h: &mut u64) {
        let value: u8 = input(n + 0);
        let out = vdupq_n_u8(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_s16(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *const i16;
        let out = vld1q_s16(ptr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_u16(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *const u16;
        let out = vld1q_u16(ptr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_u32(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *const u32;
        let out = vld1q_u32(ptr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_u8(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *const u8;
        let out = vld1q_u8(ptr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmaxq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vmaxq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevminq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vminq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmulq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vmulq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmulq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vmulq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevorrq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vorrq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_u32_1(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let out = vshlq_n_u32::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_u32_31(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let out = vshlq_n_u32::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s16_1(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let out = vshrq_n_s16::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s16_15(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let out = vshrq_n_s16::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s16_16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let out = vshrq_n_s16::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s32_1(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let out = vshrq_n_s32::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s32_31(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let out = vshrq_n_s32::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s32_32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let out = vshrq_n_s32::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevst1q_s16(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *mut i16;
        let a: int16x8_t = input(n + 5);
        vst1q_s16(ptr, a);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevst1q_u32(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *mut u32;
        let a: uint32x4_t = input(n + 5);
        vst1q_u32(ptr, a);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevsubq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vsubq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevsubq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vsubq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip1q_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vzip1q_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip2q_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vzip2q_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevabsq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let out = vabsq_s16(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevabsq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let out = vabsq_s32(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevabsq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let out = vabsq_s8(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevaddq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let b: float32x4_t = float_input(n + 5);
        let out = vaddq_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevaddq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vaddq_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevaddq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vaddq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevaddq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vaddq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevaddq_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vaddq_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevaddq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vaddq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevandq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vandq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevandq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vandq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevandq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vandq_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevandq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vandq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevandq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vandq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevandq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vandq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevandq_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vandq_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevandq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vandq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevbicq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vbicq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevceqq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let b: float32x4_t = float_input(n + 5);
        let out = vceqq_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevceqq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vceqq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevceqq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vceqq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevceqq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vceqq_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevceqq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vceqq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevceqq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vceqq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevceqq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vceqq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevceqq_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vceqq_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevceqq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vceqq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcgtq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let b: float32x4_t = float_input(n + 5);
        let out = vcgtq_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcgtq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vcgtq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcgtq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vcgtq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcgtq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vcgtq_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcgtq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vcgtq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcltq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let b: float32x4_t = float_input(n + 5);
        let out = vcltq_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcltq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vcltq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcltq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vcltq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcltq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vcltq_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcltq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vcltq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdivq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let b: float32x4_t = float_input(n + 5);
        let out = vdivq_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_f32(n: usize, h: &mut u64) {
        let value: f32 = float_input(n + 0);
        let out = vdupq_n_f32(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_f64(n: usize, h: &mut u64) {
        let value: f64 = input(n + 0);
        let out = vdupq_n_f64(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_s64(n: usize, h: &mut u64) {
        let value: i64 = input(n + 0);
        let out = vdupq_n_s64(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_s8(n: usize, h: &mut u64) {
        let value: i8 = input(n + 0);
        let out = vdupq_n_s8(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevdupq_n_u64(n: usize, h: &mut u64) {
        let value: u64 = input(n + 0);
        let out = vdupq_n_u64(value);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probeveorq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = veorq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probeveorq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = veorq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probeveorq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = veorq_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probeveorq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = veorq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probeveorq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = veorq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probeveorq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = veorq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probeveorq_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = veorq_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probeveorq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = veorq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_f32(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *const f32;
        let out = vld1q_f32(ptr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_f64(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *const f64;
        let out = vld1q_f64(ptr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_s32(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *const i32;
        let out = vld1q_s32(ptr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_s8(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *const i8;
        let out = vld1q_s8(ptr);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmaxq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vmaxq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmaxq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vmaxq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmaxq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vmaxq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmaxq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vmaxq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevminq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vminq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevminq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vminq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevminq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vminq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevminq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vminq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevminq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vminq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmulq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let b: float32x4_t = float_input(n + 5);
        let out = vmulq_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmulq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vmulq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmulq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vmulq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevorrq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vorrq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevorrq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vorrq_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevorrq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vorrq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevorrq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vorrq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevorrq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vorrq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevorrq_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vorrq_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevorrq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vorrq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqaddq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vqaddq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqaddq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vqaddq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqaddq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vqaddq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqaddq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vqaddq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqsubq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vqsubq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqsubq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vqsubq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqsubq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vqsubq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqsubq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vqsubq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_s32_1(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let out = vshlq_n_s32::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_s32_31(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let out = vshlq_n_s32::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u32_1(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let out = vshrq_n_u32::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u32_31(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let out = vshrq_n_u32::<31>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u32_32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let out = vshrq_n_u32::<32>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevst1q_f32(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *mut f32;
        let a: float32x4_t = float_input(n + 5);
        vst1q_f32(ptr, a);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevst1q_f64(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *mut f64;
        let a: float64x2_t = input(n + 5);
        vst1q_f64(ptr, a);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevst1q_s32(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *mut i32;
        let a: int32x4_t = input(n + 5);
        vst1q_s32(ptr, a);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevst1q_u8(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let ptr = buf.0.as_mut_ptr().add(16) as *mut u8;
        let a: uint8x16_t = input(n + 5);
        vst1q_u8(ptr, a);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevsubq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let b: float32x4_t = float_input(n + 5);
        let out = vsubq_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevsubq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vsubq_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevsubq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vsubq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevsubq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vsubq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevsubq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vsubq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevsubq_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vsubq_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevsubq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vsubq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip2q_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vzip2q_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqmovn_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let out = vqmovn_s32(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqmovn_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let out = vqmovn_u16(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevabsq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let out = vabsq_s64(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcgtq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vcgtq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcgtq_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vcgtq_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqmovn_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let out = vqmovn_s16(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqmovun_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let out = vqmovun_s16(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqtbl1q_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vqtbl1q_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevqtbl1q_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vqtbl1q_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcgtq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vcgtq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcgtq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vcgtq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcltq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vcltq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcltq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vcltq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcltq_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vcltq_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevcltq_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vcltq_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevrhaddq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vrhaddq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevrhaddq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vrhaddq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevabsq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let out = vabsq_f32(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevsqrtq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let out = vsqrtq_f32(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevminq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = minmax_input(n % 16);
        let b: float32x4_t = minmax_input(n / 16);
        let out = vminq_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevmaxq_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = minmax_input(n % 16);
        let b: float32x4_t = minmax_input(n / 16);
        let out = vmaxq_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip1q_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let b: float32x4_t = float_input(n + 5);
        let out = vzip1q_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip2q_f32(n: usize, h: &mut u64) {
        let a: float32x4_t = float_input(n + 0);
        let b: float32x4_t = float_input(n + 5);
        let out = vzip2q_f32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevbicq_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vbicq_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip1q_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vzip1q_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip2q_s8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let b: int8x16_t = input(n + 5);
        let out = vzip2q_s8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_s8_1(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let out = vshlq_n_s8::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_s8_7(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let out = vshlq_n_s8::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s8_1(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let out = vshrq_n_s8::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s8_7(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let out = vshrq_n_s8::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s8_8(n: usize, h: &mut u64) {
        let a: int8x16_t = input(n + 0);
        let out = vshrq_n_s8::<8>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevst1q_s8(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let a = buf.0.as_mut_ptr().add(16) as *mut i8;
        let b: int8x16_t = input(n + 5);
        vst1q_s8(a, b);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevbicq_s16(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let b: int16x8_t = input(n + 5);
        let out = vbicq_s16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_s16_1(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let out = vshlq_n_s16::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_s16_15(n: usize, h: &mut u64) {
        let a: int16x8_t = input(n + 0);
        let out = vshlq_n_s16::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevbicq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vbicq_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip1q_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vzip1q_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip2q_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n + 0);
        let b: int32x4_t = input(n + 5);
        let out = vzip2q_s32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevbicq_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vbicq_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip1q_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vzip1q_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip2q_s64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let b: int64x2_t = input(n + 5);
        let out = vzip2q_s64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_s64_1(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let out = vshlq_n_s64::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_s64_63(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let out = vshlq_n_s64::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s64_1(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let out = vshrq_n_s64::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s64_63(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let out = vshrq_n_s64::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_s64_64(n: usize, h: &mut u64) {
        let a: int64x2_t = input(n + 0);
        let out = vshrq_n_s64::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_s64(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let a = buf.0.as_mut_ptr().add(16) as *const i64;
        let out = vld1q_s64(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevst1q_s64(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let a = buf.0.as_mut_ptr().add(16) as *mut i64;
        let b: int64x2_t = input(n + 5);
        vst1q_s64(a, b);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevbicq_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vbicq_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip1q_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vzip1q_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip2q_u8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let b: uint8x16_t = input(n + 5);
        let out = vzip2q_u8(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_u8_1(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let out = vshlq_n_u8::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_u8_7(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let out = vshlq_n_u8::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u8_1(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let out = vshrq_n_u8::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u8_7(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let out = vshrq_n_u8::<7>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u8_8(n: usize, h: &mut u64) {
        let a: uint8x16_t = input(n + 0);
        let out = vshrq_n_u8::<8>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevbicq_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vbicq_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip1q_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vzip1q_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip2q_u16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let b: uint16x8_t = input(n + 5);
        let out = vzip2q_u16(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_u16_1(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let out = vshlq_n_u16::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_u16_15(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let out = vshlq_n_u16::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u16_1(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let out = vshrq_n_u16::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u16_15(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let out = vshrq_n_u16::<15>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u16_16(n: usize, h: &mut u64) {
        let a: uint16x8_t = input(n + 0);
        let out = vshrq_n_u16::<16>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevst1q_u16(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let a = buf.0.as_mut_ptr().add(16) as *mut u16;
        let b: uint16x8_t = input(n + 5);
        vst1q_u16(a, b);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevzip1q_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vzip1q_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip2q_u32(n: usize, h: &mut u64) {
        let a: uint32x4_t = input(n + 0);
        let b: uint32x4_t = input(n + 5);
        let out = vzip2q_u32(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevbicq_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vbicq_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevzip1q_u64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let b: uint64x2_t = input(n + 5);
        let out = vzip1q_u64(a, b);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_u64_1(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let out = vshlq_n_u64::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshlq_n_u64_63(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let out = vshlq_n_u64::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u64_1(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let out = vshrq_n_u64::<1>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u64_63(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let out = vshrq_n_u64::<63>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevshrq_n_u64_64(n: usize, h: &mut u64) {
        let a: uint64x2_t = input(n + 0);
        let out = vshrq_n_u64::<64>(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevld1q_u64(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let a = buf.0.as_mut_ptr().add(16) as *const u64;
        let out = vld1q_u64(a);
        mix(h, &out);
    }

    #[inline(never)]
    unsafe fn probevst1q_u64(n: usize, h: &mut u64) {
        #[repr(C, align(16))]
        struct Buffer([u8; 48]);
        let mut buf = Buffer([0xa5u8; 48]);
        let src: [u8; 16] = input(n);
        for i in 0..16 {
            buf.0[16 + i] = src[i];
        }
        let a = buf.0.as_mut_ptr().add(16) as *mut u64;
        let b: uint64x2_t = input(n + 5);
        vst1q_u64(a, b);
        mix(h, &buf.0);
    }

    #[inline(never)]
    unsafe fn probevmaxq_s32(n: usize, h: &mut u64) {
        let a: int32x4_t = input(n);
        let b: int32x4_t = input(n + 5);
        let out = vmaxq_s32(a, b);
        mix(h, &out);
    }

    #[test]
    fn simd_intrinsics() {
        // Warm every probe before the measured pass so asynchronous O3 builds
        // can finish while the other probes run. The printed hashes stay stable.
        simd_hashes(false);
        simd_hashes(true);
    }

    #[inline(never)]
    fn simd_hashes(report: bool) {
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevaddq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vaddq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevaddq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vaddq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevaddq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vaddq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmaxq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmaxq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevminq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vminq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmulq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmulq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmulq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmulq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevorrq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vorrq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_u32_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_u32_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_u32_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_u32_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s16_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s16_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s16_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s16_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s16_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s16_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s32_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s32_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s32_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s32_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s32_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s32_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsubq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsubq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsubq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsubq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip1q_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip1q_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip2q_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip2q_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevabsq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vabsq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevabsq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vabsq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevabsq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vabsq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevaddq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vaddq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevaddq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vaddq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevaddq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vaddq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevaddq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vaddq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevaddq_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vaddq_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevaddq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vaddq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevandq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vandq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevandq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vandq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevandq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vandq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevandq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vandq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevandq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vandq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevandq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vandq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevandq_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vandq_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevandq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vandq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevbicq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vbicq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevceqq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vceqq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevceqq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vceqq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevceqq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vceqq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevceqq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vceqq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevceqq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vceqq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevceqq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vceqq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevceqq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vceqq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevceqq_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vceqq_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevceqq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vceqq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcgtq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcgtq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcgtq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcgtq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcgtq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcgtq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcgtq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcgtq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcgtq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcgtq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcltq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcltq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcltq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcltq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcltq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcltq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcltq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcltq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcltq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcltq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdivq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdivq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_f64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_f64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevdupq_n_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vdupq_n_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probeveorq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD veorq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probeveorq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD veorq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probeveorq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD veorq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probeveorq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD veorq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probeveorq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD veorq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probeveorq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD veorq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probeveorq_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD veorq_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probeveorq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD veorq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_f64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_f64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmaxq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmaxq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmaxq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmaxq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmaxq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmaxq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmaxq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmaxq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevminq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vminq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevminq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vminq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevminq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vminq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevminq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vminq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevminq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vminq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmulq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmulq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmulq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmulq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmulq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmulq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevorrq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vorrq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevorrq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vorrq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevorrq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vorrq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevorrq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vorrq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevorrq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vorrq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevorrq_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vorrq_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevorrq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vorrq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqaddq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqaddq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqaddq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqaddq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqaddq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqaddq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqaddq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqaddq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqsubq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqsubq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqsubq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqsubq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqsubq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqsubq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqsubq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqsubq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_s32_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_s32_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_s32_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_s32_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u32_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u32_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u32_31(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u32_31 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u32_32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u32_32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_f64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_f64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsubq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsubq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsubq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsubq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsubq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsubq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsubq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsubq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsubq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsubq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsubq_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsubq_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsubq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsubq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip2q_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip2q_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqmovn_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqmovn_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqmovn_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqmovn_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevabsq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vabsq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcgtq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcgtq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcgtq_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcgtq_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqmovn_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqmovn_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqmovun_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqmovun_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqtbl1q_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqtbl1q_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevqtbl1q_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vqtbl1q_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcgtq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcgtq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcgtq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcgtq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcltq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcltq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcltq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcltq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcltq_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcltq_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevcltq_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vcltq_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevrhaddq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vrhaddq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevrhaddq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vrhaddq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevabsq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vabsq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevsqrtq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vsqrtq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevminq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vminq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmaxq_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmaxq_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip1q_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip1q_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip2q_f32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip2q_f32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevbicq_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vbicq_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip1q_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip1q_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip2q_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip2q_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_s8_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_s8_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_s8_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_s8_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s8_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s8_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s8_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s8_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s8_8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s8_8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_s8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_s8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevbicq_s16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vbicq_s16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_s16_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_s16_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_s16_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_s16_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevbicq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vbicq_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip1q_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip1q_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip2q_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip2q_s32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevbicq_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vbicq_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip1q_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip1q_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip2q_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip2q_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_s64_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_s64_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_s64_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_s64_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s64_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s64_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s64_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s64_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_s64_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_s64_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_s64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_s64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevbicq_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vbicq_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip1q_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip1q_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip2q_u8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip2q_u8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_u8_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_u8_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_u8_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_u8_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u8_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u8_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u8_7(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u8_7 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u8_8(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u8_8 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevbicq_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vbicq_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip1q_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip1q_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip2q_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip2q_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_u16_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_u16_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_u16_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_u16_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u16_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u16_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u16_15(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u16_15 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u16_16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u16_16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_u16(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_u16 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip1q_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip1q_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip2q_u32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip2q_u32 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevbicq_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vbicq_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevzip1q_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vzip1q_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_u64_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_u64_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshlq_n_u64_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshlq_n_u64_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u64_1(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u64_1 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u64_63(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u64_63 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevshrq_n_u64_64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vshrq_n_u64_64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevld1q_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vld1q_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevst1q_u64(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vst1q_u64 {:016x}", h);
        }
        let mut h = 0xcbf29ce484222325u64;
        for n in 0..512 {
            unsafe {
                probevmaxq_s32(core::hint::black_box(n), &mut h);
            }
        }
        if report {
            println!("SIMD vmaxq_s32 {:016x}", h);
        }
    }
}
