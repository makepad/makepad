// Differential output: rustc --test -O, Relax O1, and Relax O3.
#![allow(unused_unsafe)]
#[cfg(target_arch = "x86_64")]
#[inline(never)]
fn features() -> u64 {
    let mut bits = 0u64;
    bits |= (std::is_x86_feature_detected!("sse") as u64) << 0;
    bits |= (std::is_x86_feature_detected!("sse2") as u64) << 1;
    bits |= (std::is_x86_feature_detected!("sse3") as u64) << 2;
    bits |= (std::is_x86_feature_detected!("ssse3") as u64) << 3;
    bits |= (std::is_x86_feature_detected!("sse4.1") as u64) << 4;
    bits |= (std::is_x86_feature_detected!("sse4.2") as u64) << 5;
    bits |= (std::is_x86_feature_detected!("popcnt") as u64) << 6;
    bits |= (std::is_x86_feature_detected!("avx") as u64) << 7;
    bits |= (std::is_x86_feature_detected!("avx2") as u64) << 8;
    bits |= (std::is_x86_feature_detected!("fma") as u64) << 9;
    bits |= (std::is_x86_feature_detected!("f16c") as u64) << 10;
    bits |= (std::is_x86_feature_detected!("bmi1") as u64) << 11;
    bits |= (std::is_x86_feature_detected!("bmi2") as u64) << 12;
    bits |= (std::is_x86_feature_detected!("lzcnt") as u64) << 13;
    bits |= (std::is_x86_feature_detected!("pclmulqdq") as u64) << 14;
    bits |= (std::is_x86_feature_detected!("sha") as u64) << 15;
    bits |= (std::is_x86_feature_detected!("aes") as u64) << 16;
    bits |= (std::is_x86_feature_detected!("avx512f") as u64) << 17;
    bits |= (std::is_x86_feature_detected!("avx512bw") as u64) << 18;
    bits |= (std::is_x86_feature_detected!("avx512bf16") as u64) << 19;
    bits |= (std::is_x86_feature_detected!("avx512fp16") as u64) << 20;
    bits |= (std::is_x86_feature_detected!("ermsb") as u64) << 21;
    bits
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
fn features() -> u64 {
    let mut bits = 0u64;
    bits |= (std::arch::is_aarch64_feature_detected!("neon") as u64) << 0;
    bits |= (std::arch::is_aarch64_feature_detected!("fp") as u64) << 1;
    bits |= (std::arch::is_aarch64_feature_detected!("crc") as u64) << 2;
    bits |= (std::arch::is_aarch64_feature_detected!("aes") as u64) << 3;
    bits |= (std::arch::is_aarch64_feature_detected!("pmull") as u64) << 4;
    bits |= (std::arch::is_aarch64_feature_detected!("sha2") as u64) << 5;
    bits |= (std::arch::is_aarch64_feature_detected!("sha3") as u64) << 6;
    bits |= (std::arch::is_aarch64_feature_detected!("fp16") as u64) << 7;
    bits |= (std::arch::is_aarch64_feature_detected!("fhm") as u64) << 8;
    bits |= (std::arch::is_aarch64_feature_detected!("dotprod") as u64) << 9;
    bits |= (std::arch::is_aarch64_feature_detected!("lse") as u64) << 10;
    bits |= (std::arch::is_aarch64_feature_detected!("rdm") as u64) << 11;
    bits |= (std::arch::is_aarch64_feature_detected!("i8mm") as u64) << 12;
    bits |= (std::arch::is_aarch64_feature_detected!("bf16") as u64) << 13;
    bits
}

#[test]
fn cpu_features() {
    let expected = features();
    // Cross the O3 call threshold and exercise the cached detector.
    for _ in 0..256 {
        assert_eq!(features(), expected);
    }
    #[cfg(target_arch = "x86_64")]
    {
        println!("feature sse={}", expected & (1 << 0) != 0);
        println!("feature sse2={}", expected & (1 << 1) != 0);
        println!("feature sse3={}", expected & (1 << 2) != 0);
        println!("feature ssse3={}", expected & (1 << 3) != 0);
        println!("feature sse4.1={}", expected & (1 << 4) != 0);
        println!("feature sse4.2={}", expected & (1 << 5) != 0);
        println!("feature popcnt={}", expected & (1 << 6) != 0);
        println!("feature avx={}", expected & (1 << 7) != 0);
        println!("feature avx2={}", expected & (1 << 8) != 0);
        println!("feature fma={}", expected & (1 << 9) != 0);
        println!("feature f16c={}", expected & (1 << 10) != 0);
        println!("feature bmi1={}", expected & (1 << 11) != 0);
        println!("feature bmi2={}", expected & (1 << 12) != 0);
        println!("feature lzcnt={}", expected & (1 << 13) != 0);
        println!("feature pclmulqdq={}", expected & (1 << 14) != 0);
        println!("feature sha={}", expected & (1 << 15) != 0);
        println!("feature aes={}", expected & (1 << 16) != 0);
        println!("feature avx512f={}", expected & (1 << 17) != 0);
        println!("feature avx512bw={}", expected & (1 << 18) != 0);
        println!("feature avx512bf16={}", expected & (1 << 19) != 0);
        println!("feature avx512fp16={}", expected & (1 << 20) != 0);
        println!("feature ermsb={}", expected & (1 << 21) != 0);
    }
    #[cfg(target_arch = "aarch64")]
    {
        println!("feature neon={}", expected & (1 << 0) != 0);
        println!("feature fp={}", expected & (1 << 1) != 0);
        println!("feature crc={}", expected & (1 << 2) != 0);
        println!("feature aes={}", expected & (1 << 3) != 0);
        println!("feature pmull={}", expected & (1 << 4) != 0);
        println!("feature sha2={}", expected & (1 << 5) != 0);
        println!("feature sha3={}", expected & (1 << 6) != 0);
        println!("feature fp16={}", expected & (1 << 7) != 0);
        println!("feature fhm={}", expected & (1 << 8) != 0);
        println!("feature dotprod={}", expected & (1 << 9) != 0);
        println!("feature lse={}", expected & (1 << 10) != 0);
        println!("feature rdm={}", expected & (1 << 11) != 0);
        println!("feature i8mm={}", expected & (1 << 12) != 0);
        println!("feature bf16={}", expected & (1 << 13) != 0);
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(never)]
fn query(leaf: u32, subleaf: u32, values: [u64; 12]) -> (core::arch::x86_64::CpuidResult, u64) {
    // Keep integer values live across the four fixed-register clobbers.
    let [a, b, c, d, e, f, g, h, i, j, k, l] = values;
    let r = unsafe { core::arch::x86_64::__cpuid_count(leaf, subleaf) };
    (r, a ^ b ^ c ^ d ^ e ^ f ^ g ^ h ^ i ^ j ^ k ^ l)
}

#[cfg(target_arch = "x86_64")]
#[test]
fn cpuid_vendor() {
    use core::arch::x86_64::{__cpuid, __get_cpuid_max, _xgetbv};
    unsafe {
        let r = __cpuid(0);
        assert!(r.eax >= 1);
        let mut vendor = [0u8; 12];
        vendor[0..4].copy_from_slice(&r.ebx.to_le_bytes());
        vendor[4..8].copy_from_slice(&r.edx.to_le_bytes());
        vendor[8..12].copy_from_slice(&r.ecx.to_le_bytes());
        let vendor = core::str::from_utf8(&vendor).unwrap();
        assert_eq!(vendor.len(), 12);
        println!("vendor {}", vendor);
        assert_eq!(__get_cpuid_max(0), (r.eax, r.ebx));
        let ext = __cpuid(0x80000000);
        assert_eq!(__get_cpuid_max(0x80000000), (ext.eax, ext.ebx));
        for n in 0..256u64 {
            let values = [
                n,
                n + 1,
                n + 2,
                n + 3,
                n + 4,
                n + 5,
                n + 6,
                n + 7,
                n + 8,
                n + 9,
                n + 10,
                n + 11,
            ];
            let mut expected = 0;
            for v in values {
                expected ^= v;
            }
            let (got, live) = query(0, 0, values);
            assert_eq!(got, r);
            assert_eq!(live, expected);
            if r.eax >= 7 {
                let (got, live) = query(7, 0, values);
                assert_eq!(got, __cpuid(7));
                assert_eq!(live, expected);
                if got.eax >= 1 {
                    let _ = query(7, 1, values);
                }
            }
        }
        let p = __cpuid(1);
        if p.ecx & (3 << 26) == (3 << 26) {
            let xcr0 = _xgetbv(0);
            assert!(xcr0 & 1 != 0);
            if std::is_x86_feature_detected!("avx") {
                assert_eq!(xcr0 & 6, 6);
            }
            println!("xcr0 {}", xcr0);
        }
    }
}
