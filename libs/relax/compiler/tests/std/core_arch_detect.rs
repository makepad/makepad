// std-core differential test (core_diff.sh): run-time feature detection and SIMD intrinsics
// (aarch64: NEON + CRC; x86_64: SSE2 + feature bits).
#[cfg(target_arch = "aarch64")]
#[test]
fn a01() {
    println!("| neon {}", std::arch::is_aarch64_feature_detected!("neon"));
    println!("| crc {} sha2 {}", std::arch::is_aarch64_feature_detected!("crc"), std::arch::is_aarch64_feature_detected!("sha2"));
    unsafe {
        use std::arch::aarch64::*;
        let a = vdupq_n_u32(3);
        let b = vaddq_u32(a, vdupq_n_u32(4));
        println!("| lane {}", vgetq_lane_u32::<2>(b));
        println!("| crc32 {:x}", __crc32d(0, 0x1234_5678_9abc_def0));
    }
}
#[cfg(target_arch = "x86_64")]
#[test]
fn a01() {
    println!("| sse2 {} avx2 {} sha {}", std::arch::is_x86_feature_detected!("sse2"), std::arch::is_x86_feature_detected!("avx2"), std::arch::is_x86_feature_detected!("sha"));
    unsafe {
        use std::arch::x86_64::*;
        let a = _mm_set1_epi16(300);
        let b = _mm_add_epi16(a, _mm_set1_epi16(-5));
        println!("| lane {}", _mm_cvtsi128_si32(b));
    }
}
