// core::arch::x86_64 over the same inputs as real std's: rustc builds it on x86_64 hosts only, where agree.sh
// compares its `| ` lines with makepad test's. Relax also has core::arch::x86_64 on Arm hosts (emulated, every
// feature present): there makepad test's lines are compared with an x86_64 host's real ones,
// arch/x86_64_real.txt (an agree across hosts).
// Each immediate's `compare` stays a function of its own (inline(never)): 256 of them inlined into
// one loop overflowed rustc -O's stack in LLVM's known-bits/SCEV recursion.
#![allow(non_snake_case)]
include!("arch/common.rs");
use core::arch::x86_64 as real;

#[path = "arch/intel.rs"]
mod intel;

/// AVX-512 as this CPU and its OS give it (CPUID and XCR0, as std's detection reads them; `bw`
/// asks for AVX512BW too), for the tests that run real AVX-512 code. Not is_x86_feature_detected:
/// Relax's std reports no AVX-512 while its asm! assembler encodes no EVEX, though the intrinsics
/// here compute, so on an AVX-512 host the two runs took different branches.
unsafe fn cpu_avx512(bw: bool) -> bool {
    if real::__get_cpuid_max(0).0 < 7 || real::__cpuid(1).ecx & (1 << 27) == 0 {
        return false;
    }
    // the OS saves the AVX state (XCR0 bits 1-2) and the opmask and ZMM state (bits 5-7)
    let ebx = real::__cpuid_count(7, 0).ebx;
    real::_xgetbv(0) & 0xe6 == 0xe6 && ebx & (1 << 16) != 0 && (!bw || ebx & (1 << 30) != 0)
}

#[test]
fn a001__mm256_add_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_add_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_add_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_add_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_add_epi16", "none");
}

#[test]
fn a002__mm256_add_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_add_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_add_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_add_epi32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_add_epi32", "none");
}

#[test]
fn a003__mm256_blend_epi32() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, p1, _case, d): (real::__m256i, real::__m256i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm256_blend_epi32::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_blend_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_blend_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            immediate_cases!(
                compare,
                (p0, p1, case, &mut d),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
                    42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
                    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                    82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
                    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
                    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132,
                    133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148,
                    149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164,
                    165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180,
                    181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196,
                    197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212,
                    213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228,
                    229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244,
                    245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
                ]
            );
        }
    }
    d.line("_mm256_blend_epi32", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,198,199,200,201,202,203,204,205,206,207,208,209,210,211,212,213,214,215,216,217,218,219,220,221,222,223,224,225,226,227,228,229,230,231,232,233,234,235,236,237,238,239,240,241,242,243,244,245,246,247,248,249,250,251,252,253,254,255");
}

#[test]
fn a004__mm256_blendv_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_blendv_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_blendv_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let p2: real::__m256i = sample(&mut rng, case, 2);
            let expected = real::_mm256_blendv_epi8(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_blendv_epi8", "none");
}

#[test]
fn a005__mm256_castsi256_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_castsi256_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_castsi256_si128: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let expected = real::_mm256_castsi256_si128(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_castsi256_si128", "none");
}

#[test]
fn a006__mm256_extractf128_si256() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m256i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm256_extractf128_si256::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_extractf128_si256();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_extractf128_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case, &mut d), [0, 1]);
        }
    }
    d.line("_mm256_extractf128_si256", "0,1");
}

#[test]
fn a007__mm256_extracti128_si256() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m256i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm256_extracti128_si256::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_extracti128_si256();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_extracti128_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case, &mut d), [0, 1]);
        }
    }
    d.line("_mm256_extracti128_si256", "0,1");
}

#[test]
fn a008__mm256_loadu_si256() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_loadu_si256();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_loadu_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::_mm256_loadu_si256(p0.0.as_ptr().add(case % 16).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_loadu_si256", "none");
}

#[test]
fn a009__mm256_madd_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_madd_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_madd_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_madd_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_madd_epi16", "none");
}

#[test]
fn a010__mm256_maddubs_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_maddubs_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_maddubs_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_maddubs_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_maddubs_epi16", "none");
}

#[test]
fn a011__mm256_max_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_max_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_max_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_max_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_max_epi16", "none");
}

#[test]
fn a012__mm256_min_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_min_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_min_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_min_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_min_epi16", "none");
}

#[test]
fn a013__mm256_mullo_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_mullo_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_mullo_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_mullo_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_mullo_epi16", "none");
}

#[test]
fn a014__mm256_mullo_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_mullo_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_mullo_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_mullo_epi32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_mullo_epi32", "none");
}

#[test]
fn a015__mm256_or_si256() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_or_si256();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_or_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_or_si256(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_or_si256", "none");
}

#[test]
fn a016__mm256_packs_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_packs_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_packs_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_packs_epi32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_packs_epi32", "none");
}

#[test]
fn a017__mm256_packus_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_packus_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_packus_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_packus_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_packus_epi16", "none");
}

#[test]
fn a018__mm256_packus_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_packus_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_packus_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_packus_epi32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_packus_epi32", "none");
}

#[test]
fn a019__mm256_permute2x128_si256() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, p1, _case, d): (real::__m256i, real::__m256i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm256_permute2x128_si256::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_permute2x128_si256();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_permute2x128_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            immediate_cases!(
                compare,
                (p0, p1, case, &mut d),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
                    42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
                    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                    82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
                    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
                    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132,
                    133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148,
                    149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164,
                    165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180,
                    181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196,
                    197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212,
                    213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228,
                    229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244,
                    245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
                ]
            );
        }
    }
    d.line("_mm256_permute2x128_si256", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,198,199,200,201,202,203,204,205,206,207,208,209,210,211,212,213,214,215,216,217,218,219,220,221,222,223,224,225,226,227,228,229,230,231,232,233,234,235,236,237,238,239,240,241,242,243,244,245,246,247,248,249,250,251,252,253,254,255");
}

#[test]
fn a020__mm256_permute4x64_epi64() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m256i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm256_permute4x64_epi64::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_permute4x64_epi64();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_permute4x64_epi64: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
                    42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
                    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                    82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
                    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
                    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132,
                    133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148,
                    149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164,
                    165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180,
                    181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196,
                    197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212,
                    213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228,
                    229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244,
                    245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
                ]
            );
        }
    }
    d.line("_mm256_permute4x64_epi64", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,198,199,200,201,202,203,204,205,206,207,208,209,210,211,212,213,214,215,216,217,218,219,220,221,222,223,224,225,226,227,228,229,230,231,232,233,234,235,236,237,238,239,240,241,242,243,244,245,246,247,248,249,250,251,252,253,254,255");
}

#[test]
fn a021__mm256_sad_epu8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_sad_epu8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_sad_epu8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_sad_epu8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_sad_epu8", "none");
}

#[test]
fn a022__mm256_set1_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_set1_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_set1_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i16 = sample(&mut rng, case, 0);
            let expected = real::_mm256_set1_epi16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_set1_epi16", "none");
}

#[test]
fn a023__mm256_set1_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_set1_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_set1_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let expected = real::_mm256_set1_epi32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_set1_epi32", "none");
}

#[test]
fn a024__mm256_set_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_set_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_set_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let p1: i32 = sample(&mut rng, case, 1);
            let p2: i32 = sample(&mut rng, case, 2);
            let p3: i32 = sample(&mut rng, case, 3);
            let p4: i32 = sample(&mut rng, case, 4);
            let p5: i32 = sample(&mut rng, case, 5);
            let p6: i32 = sample(&mut rng, case, 6);
            let p7: i32 = sample(&mut rng, case, 7);
            let expected = real::_mm256_set_epi32(p0, p1, p2, p3, p4, p5, p6, p7);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_set_epi32", "none");
}

#[test]
fn a025__mm256_set_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_set_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_set_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let p1: i8 = sample(&mut rng, case, 1);
            let p2: i8 = sample(&mut rng, case, 2);
            let p3: i8 = sample(&mut rng, case, 3);
            let p4: i8 = sample(&mut rng, case, 4);
            let p5: i8 = sample(&mut rng, case, 5);
            let p6: i8 = sample(&mut rng, case, 6);
            let p7: i8 = sample(&mut rng, case, 7);
            let p8: i8 = sample(&mut rng, case, 8);
            let p9: i8 = sample(&mut rng, case, 9);
            let p10: i8 = sample(&mut rng, case, 10);
            let p11: i8 = sample(&mut rng, case, 11);
            let p12: i8 = sample(&mut rng, case, 12);
            let p13: i8 = sample(&mut rng, case, 13);
            let p14: i8 = sample(&mut rng, case, 14);
            let p15: i8 = sample(&mut rng, case, 15);
            let p16: i8 = sample(&mut rng, case, 16);
            let p17: i8 = sample(&mut rng, case, 17);
            let p18: i8 = sample(&mut rng, case, 18);
            let p19: i8 = sample(&mut rng, case, 19);
            let p20: i8 = sample(&mut rng, case, 20);
            let p21: i8 = sample(&mut rng, case, 21);
            let p22: i8 = sample(&mut rng, case, 22);
            let p23: i8 = sample(&mut rng, case, 23);
            let p24: i8 = sample(&mut rng, case, 24);
            let p25: i8 = sample(&mut rng, case, 25);
            let p26: i8 = sample(&mut rng, case, 26);
            let p27: i8 = sample(&mut rng, case, 27);
            let p28: i8 = sample(&mut rng, case, 28);
            let p29: i8 = sample(&mut rng, case, 29);
            let p30: i8 = sample(&mut rng, case, 30);
            let p31: i8 = sample(&mut rng, case, 31);
            let expected = real::_mm256_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_set_epi8", "none");
}

#[test]
fn a026__mm256_setr_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_setr_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_setr_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let p1: i8 = sample(&mut rng, case, 1);
            let p2: i8 = sample(&mut rng, case, 2);
            let p3: i8 = sample(&mut rng, case, 3);
            let p4: i8 = sample(&mut rng, case, 4);
            let p5: i8 = sample(&mut rng, case, 5);
            let p6: i8 = sample(&mut rng, case, 6);
            let p7: i8 = sample(&mut rng, case, 7);
            let p8: i8 = sample(&mut rng, case, 8);
            let p9: i8 = sample(&mut rng, case, 9);
            let p10: i8 = sample(&mut rng, case, 10);
            let p11: i8 = sample(&mut rng, case, 11);
            let p12: i8 = sample(&mut rng, case, 12);
            let p13: i8 = sample(&mut rng, case, 13);
            let p14: i8 = sample(&mut rng, case, 14);
            let p15: i8 = sample(&mut rng, case, 15);
            let p16: i8 = sample(&mut rng, case, 16);
            let p17: i8 = sample(&mut rng, case, 17);
            let p18: i8 = sample(&mut rng, case, 18);
            let p19: i8 = sample(&mut rng, case, 19);
            let p20: i8 = sample(&mut rng, case, 20);
            let p21: i8 = sample(&mut rng, case, 21);
            let p22: i8 = sample(&mut rng, case, 22);
            let p23: i8 = sample(&mut rng, case, 23);
            let p24: i8 = sample(&mut rng, case, 24);
            let p25: i8 = sample(&mut rng, case, 25);
            let p26: i8 = sample(&mut rng, case, 26);
            let p27: i8 = sample(&mut rng, case, 27);
            let p28: i8 = sample(&mut rng, case, 28);
            let p29: i8 = sample(&mut rng, case, 29);
            let p30: i8 = sample(&mut rng, case, 30);
            let p31: i8 = sample(&mut rng, case, 31);
            let expected = real::_mm256_setr_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_setr_epi8", "none");
}

#[test]
fn a027__mm256_setzero_si256() {
    let mut d = Digest::new();
    unsafe {
        intel::_mm256_setzero_si256();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_setzero_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for _ in 0..RANDOM_CASES + 17 {
            let expected = real::_mm256_setzero_si256();
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_setzero_si256", "none");
}

#[test]
fn a028__mm256_shuffle_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_shuffle_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_shuffle_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_shuffle_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_shuffle_epi8", "none");
}

#[test]
fn a029__mm256_slli_epi16() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m256i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm256_slli_epi16::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_slli_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_slli_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
    d.line("_mm256_slli_epi16", "0,1,2,7,8,15,16,17,31,32,33,63,64,127,128,255");
}

#[test]
fn a030__mm256_slli_epi32() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m256i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm256_slli_epi32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_slli_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_slli_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
    d.line("_mm256_slli_epi32", "0,1,2,7,8,15,16,17,31,32,33,63,64,127,128,255");
}

#[test]
fn a031__mm256_srai_epi16() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m256i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm256_srai_epi16::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_srai_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_srai_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
    d.line("_mm256_srai_epi16", "0,1,2,7,8,15,16,17,31,32,33,63,64,127,128,255");
}

#[test]
fn a032__mm256_srai_epi32() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m256i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm256_srai_epi32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_srai_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_srai_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
    d.line("_mm256_srai_epi32", "0,1,2,7,8,15,16,17,31,32,33,63,64,127,128,255");
}

#[test]
fn a033__mm256_storeu_si256() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_storeu_si256();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_storeu_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            real::_mm256_storeu_si256(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            d.add(&p0.0);
        }
    }
    d.line("_mm256_storeu_si256", "none");
}

#[test]
fn a034__mm256_sub_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_sub_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_sub_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_sub_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_sub_epi16", "none");
}

#[test]
fn a035__mm256_sub_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_sub_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_sub_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_sub_epi32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_sub_epi32", "none");
}

#[test]
fn a036__mm256_testz_si256() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_testz_si256();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_testz_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_testz_si256(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_testz_si256", "none");
}

#[test]
fn a037__mm256_unpackhi_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpackhi_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpackhi_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_unpackhi_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_unpackhi_epi16", "none");
}

#[test]
fn a038__mm256_unpackhi_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpackhi_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpackhi_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_unpackhi_epi32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_unpackhi_epi32", "none");
}

#[test]
fn a039__mm256_unpackhi_epi64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpackhi_epi64();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpackhi_epi64: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_unpackhi_epi64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_unpackhi_epi64", "none");
}

#[test]
fn a040__mm256_unpackhi_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpackhi_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpackhi_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_unpackhi_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_unpackhi_epi8", "none");
}

#[test]
fn a041__mm256_unpacklo_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpacklo_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpacklo_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_unpacklo_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_unpacklo_epi16", "none");
}

#[test]
fn a042__mm256_unpacklo_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpacklo_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpacklo_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_unpacklo_epi32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_unpacklo_epi32", "none");
}

#[test]
fn a043__mm256_unpacklo_epi64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpacklo_epi64();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpacklo_epi64: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_unpacklo_epi64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_unpacklo_epi64", "none");
}

#[test]
fn a044__mm256_unpacklo_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpacklo_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpacklo_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m256i = sample(&mut rng, case, 0);
            let p1: real::__m256i = sample(&mut rng, case, 1);
            let expected = real::_mm256_unpacklo_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm256_unpacklo_epi8", "none");
}

#[test]
fn a045__mm512_add_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_add_epi32();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(false) {
            eprintln!("_mm512_add_epi32: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m512i = sample(&mut rng, case, 0);
            let p1: real::__m512i = sample(&mut rng, case, 1);
            let expected = real::_mm512_add_epi32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm512_add_epi32", "none");
}

#[test]
fn a046__mm512_loadu_si512() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_loadu_si512();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(false) {
            eprintln!("_mm512_loadu_si512: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::_mm512_loadu_si512(p0.0.as_ptr().add(case % 16).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("_mm512_loadu_si512", "none");
}

#[test]
fn a047__mm512_madd_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_madd_epi16();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(true) {
            eprintln!("_mm512_madd_epi16: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m512i = sample(&mut rng, case, 0);
            let p1: real::__m512i = sample(&mut rng, case, 1);
            let expected = real::_mm512_madd_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm512_madd_epi16", "none");
}

#[test]
fn a048__mm512_maddubs_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_maddubs_epi16();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(true) {
            eprintln!("_mm512_maddubs_epi16: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m512i = sample(&mut rng, case, 0);
            let p1: real::__m512i = sample(&mut rng, case, 1);
            let expected = real::_mm512_maddubs_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm512_maddubs_epi16", "none");
}

#[test]
fn a049__mm512_sad_epu8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_sad_epu8();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(true) {
            eprintln!("_mm512_sad_epu8: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m512i = sample(&mut rng, case, 0);
            let p1: real::__m512i = sample(&mut rng, case, 1);
            let expected = real::_mm512_sad_epu8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm512_sad_epu8", "none");
}

#[test]
fn a050__mm512_set1_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_set1_epi16();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(true) {
            eprintln!("_mm512_set1_epi16: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i16 = sample(&mut rng, case, 0);
            let expected = real::_mm512_set1_epi16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm512_set1_epi16", "none");
}

#[test]
fn a051__mm512_set_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_set_epi32();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(false) {
            eprintln!("_mm512_set_epi32: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let p1: i32 = sample(&mut rng, case, 1);
            let p2: i32 = sample(&mut rng, case, 2);
            let p3: i32 = sample(&mut rng, case, 3);
            let p4: i32 = sample(&mut rng, case, 4);
            let p5: i32 = sample(&mut rng, case, 5);
            let p6: i32 = sample(&mut rng, case, 6);
            let p7: i32 = sample(&mut rng, case, 7);
            let p8: i32 = sample(&mut rng, case, 8);
            let p9: i32 = sample(&mut rng, case, 9);
            let p10: i32 = sample(&mut rng, case, 10);
            let p11: i32 = sample(&mut rng, case, 11);
            let p12: i32 = sample(&mut rng, case, 12);
            let p13: i32 = sample(&mut rng, case, 13);
            let p14: i32 = sample(&mut rng, case, 14);
            let p15: i32 = sample(&mut rng, case, 15);
            let expected = real::_mm512_set_epi32(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            d.add(bytes(&expected));
        }
    }
    d.line("_mm512_set_epi32", "none");
}

#[test]
fn a052__mm512_set_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_set_epi8();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(true) {
            eprintln!("_mm512_set_epi8: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let p1: i8 = sample(&mut rng, case, 1);
            let p2: i8 = sample(&mut rng, case, 2);
            let p3: i8 = sample(&mut rng, case, 3);
            let p4: i8 = sample(&mut rng, case, 4);
            let p5: i8 = sample(&mut rng, case, 5);
            let p6: i8 = sample(&mut rng, case, 6);
            let p7: i8 = sample(&mut rng, case, 7);
            let p8: i8 = sample(&mut rng, case, 8);
            let p9: i8 = sample(&mut rng, case, 9);
            let p10: i8 = sample(&mut rng, case, 10);
            let p11: i8 = sample(&mut rng, case, 11);
            let p12: i8 = sample(&mut rng, case, 12);
            let p13: i8 = sample(&mut rng, case, 13);
            let p14: i8 = sample(&mut rng, case, 14);
            let p15: i8 = sample(&mut rng, case, 15);
            let p16: i8 = sample(&mut rng, case, 16);
            let p17: i8 = sample(&mut rng, case, 17);
            let p18: i8 = sample(&mut rng, case, 18);
            let p19: i8 = sample(&mut rng, case, 19);
            let p20: i8 = sample(&mut rng, case, 20);
            let p21: i8 = sample(&mut rng, case, 21);
            let p22: i8 = sample(&mut rng, case, 22);
            let p23: i8 = sample(&mut rng, case, 23);
            let p24: i8 = sample(&mut rng, case, 24);
            let p25: i8 = sample(&mut rng, case, 25);
            let p26: i8 = sample(&mut rng, case, 26);
            let p27: i8 = sample(&mut rng, case, 27);
            let p28: i8 = sample(&mut rng, case, 28);
            let p29: i8 = sample(&mut rng, case, 29);
            let p30: i8 = sample(&mut rng, case, 30);
            let p31: i8 = sample(&mut rng, case, 31);
            let p32: i8 = sample(&mut rng, case, 32);
            let p33: i8 = sample(&mut rng, case, 33);
            let p34: i8 = sample(&mut rng, case, 34);
            let p35: i8 = sample(&mut rng, case, 35);
            let p36: i8 = sample(&mut rng, case, 36);
            let p37: i8 = sample(&mut rng, case, 37);
            let p38: i8 = sample(&mut rng, case, 38);
            let p39: i8 = sample(&mut rng, case, 39);
            let p40: i8 = sample(&mut rng, case, 40);
            let p41: i8 = sample(&mut rng, case, 41);
            let p42: i8 = sample(&mut rng, case, 42);
            let p43: i8 = sample(&mut rng, case, 43);
            let p44: i8 = sample(&mut rng, case, 44);
            let p45: i8 = sample(&mut rng, case, 45);
            let p46: i8 = sample(&mut rng, case, 46);
            let p47: i8 = sample(&mut rng, case, 47);
            let p48: i8 = sample(&mut rng, case, 48);
            let p49: i8 = sample(&mut rng, case, 49);
            let p50: i8 = sample(&mut rng, case, 50);
            let p51: i8 = sample(&mut rng, case, 51);
            let p52: i8 = sample(&mut rng, case, 52);
            let p53: i8 = sample(&mut rng, case, 53);
            let p54: i8 = sample(&mut rng, case, 54);
            let p55: i8 = sample(&mut rng, case, 55);
            let p56: i8 = sample(&mut rng, case, 56);
            let p57: i8 = sample(&mut rng, case, 57);
            let p58: i8 = sample(&mut rng, case, 58);
            let p59: i8 = sample(&mut rng, case, 59);
            let p60: i8 = sample(&mut rng, case, 60);
            let p61: i8 = sample(&mut rng, case, 61);
            let p62: i8 = sample(&mut rng, case, 62);
            let p63: i8 = sample(&mut rng, case, 63);
            let expected = real::_mm512_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31, p32, p33,
                p34, p35, p36, p37, p38, p39, p40, p41, p42, p43, p44, p45, p46, p47, p48, p49,
                p50, p51, p52, p53, p54, p55, p56, p57, p58, p59, p60, p61, p62, p63,
            );
            d.add(bytes(&expected));
        }
    }
    d.line("_mm512_set_epi8", "none");
}

#[test]
fn a053__mm512_setzero_si512() {
    let mut d = Digest::new();
    unsafe {
        intel::_mm512_setzero_si512();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(false) {
            eprintln!("_mm512_setzero_si512: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for _ in 0..RANDOM_CASES + 17 {
            let expected = real::_mm512_setzero_si512();
            d.add(bytes(&expected));
        }
    }
    d.line("_mm512_setzero_si512", "none");
}

#[test]
fn a054__mm512_slli_epi32() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: u32>((p0, _case, d): (real::__m512i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm512_slli_epi32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_slli_epi32();
        if cfg!(target_arch = "x86_64") && !cpu_avx512(false) {
            eprintln!("_mm512_slli_epi32: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m512i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
    d.line("_mm512_slli_epi32", "0,1,2,7,8,15,16,17,31,32,33,63,64,127,128,255");
}

#[test]
fn a055__mm_abs_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_abs_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("ssse3") {
            eprintln!("_mm_abs_epi16: ssse3 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let expected = real::_mm_abs_epi16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_abs_epi16", "none");
}

#[test]
fn a056__mm_add_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_add_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_add_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_add_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_add_epi16", "none");
}

#[test]
fn a057__mm_add_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_add_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_add_epi32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_add_epi32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_add_epi32", "none");
}

#[test]
fn a058__mm_add_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_add_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_add_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_add_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_add_epi8", "none");
}

#[test]
fn a059__mm_and_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_and_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_and_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_and_si128(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_and_si128", "none");
}

#[test]
fn a060__mm_andnot_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_andnot_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_andnot_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_andnot_si128(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_andnot_si128", "none");
}

#[test]
fn a061__mm_avg_epu8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_avg_epu8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_avg_epu8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_avg_epu8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_avg_epu8", "none");
}

#[test]
fn a062__mm_blendv_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_blendv_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse4.1") {
            eprintln!("_mm_blendv_epi8: sse4.1 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let p2: real::__m128i = sample(&mut rng, case, 2);
            let expected = real::_mm_blendv_epi8(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_blendv_epi8", "none");
}

#[test]
fn a063__mm_clmulepi64_si128() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, p1, _case, d): (real::__m128i, real::__m128i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm_clmulepi64_si128::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_clmulepi64_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("pclmulqdq") {
            eprintln!("_mm_clmulepi64_si128: pclmulqdq unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            immediate_cases!(compare, (p0, p1, case, &mut d), [0, 1, 16, 17, 255]);
        }
    }
    d.line("_mm_clmulepi64_si128", "0,1,16,17,255");
}

#[test]
fn a064__mm_cmpeq_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cmpeq_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cmpeq_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_cmpeq_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_cmpeq_epi16", "none");
}

#[test]
fn a065__mm_cvtsi128_si32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cvtsi128_si32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cvtsi128_si32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let expected = real::_mm_cvtsi128_si32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_cvtsi128_si32", "none");
}

#[test]
fn a066__mm_cvtsi32_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cvtsi32_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cvtsi32_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let expected = real::_mm_cvtsi32_si128(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_cvtsi32_si128", "none");
}

#[test]
fn a067__mm_extract_epi32() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m128i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm_extract_epi32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_extract_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse4.1") {
            eprintln!("_mm_extract_epi32: sse4.1 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case, &mut d), [0, 1, 2, 3]);
        }
    }
    d.line("_mm_extract_epi32", "0,1,2,3");
}

#[test]
fn a068__mm_load_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_load_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_load_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::_mm_load_si128(p0.0.as_ptr().add(0).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_load_si128", "none");
}

#[test]
fn a069__mm_loadu_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_loadu_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_loadu_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::_mm_loadu_si128(p0.0.as_ptr().add(case % 16).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_loadu_si128", "none");
}

#[test]
fn a070__mm_madd_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_madd_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_madd_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_madd_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_madd_epi16", "none");
}

#[test]
fn a071__mm_maddubs_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_maddubs_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("ssse3") {
            eprintln!("_mm_maddubs_epi16: ssse3 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_maddubs_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_maddubs_epi16", "none");
}

#[test]
fn a072__mm_min_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_min_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_min_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_min_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_min_epi16", "none");
}

#[test]
fn a073__mm_or_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_or_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_or_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_or_si128(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_or_si128", "none");
}

#[test]
fn a074__mm_packus_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_packus_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_packus_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_packus_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_packus_epi16", "none");
}

#[test]
fn a075__mm_sad_epu8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_sad_epu8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_sad_epu8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_sad_epu8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_sad_epu8", "none");
}

#[test]
fn a076__mm_set1_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set1_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set1_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i16 = sample(&mut rng, case, 0);
            let expected = real::_mm_set1_epi16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_set1_epi16", "none");
}

#[test]
fn a077__mm_set1_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set1_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set1_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let expected = real::_mm_set1_epi8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_set1_epi8", "none");
}

#[test]
fn a078__mm_set_epi32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set_epi32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let p1: i32 = sample(&mut rng, case, 1);
            let p2: i32 = sample(&mut rng, case, 2);
            let p3: i32 = sample(&mut rng, case, 3);
            let expected = real::_mm_set_epi32(p0, p1, p2, p3);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_set_epi32", "none");
}

#[test]
fn a079__mm_set_epi64x() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set_epi64x();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set_epi64x: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i64 = sample(&mut rng, case, 0);
            let p1: i64 = sample(&mut rng, case, 1);
            let expected = real::_mm_set_epi64x(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_set_epi64x", "none");
}

#[test]
fn a080__mm_set_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let p1: i8 = sample(&mut rng, case, 1);
            let p2: i8 = sample(&mut rng, case, 2);
            let p3: i8 = sample(&mut rng, case, 3);
            let p4: i8 = sample(&mut rng, case, 4);
            let p5: i8 = sample(&mut rng, case, 5);
            let p6: i8 = sample(&mut rng, case, 6);
            let p7: i8 = sample(&mut rng, case, 7);
            let p8: i8 = sample(&mut rng, case, 8);
            let p9: i8 = sample(&mut rng, case, 9);
            let p10: i8 = sample(&mut rng, case, 10);
            let p11: i8 = sample(&mut rng, case, 11);
            let p12: i8 = sample(&mut rng, case, 12);
            let p13: i8 = sample(&mut rng, case, 13);
            let p14: i8 = sample(&mut rng, case, 14);
            let p15: i8 = sample(&mut rng, case, 15);
            let expected = real::_mm_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_set_epi8", "none");
}

#[test]
fn a081__mm_setzero_si128() {
    let mut d = Digest::new();
    unsafe {
        intel::_mm_setzero_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_setzero_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for _ in 0..RANDOM_CASES + 17 {
            let expected = real::_mm_setzero_si128();
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_setzero_si128", "none");
}

#[test]
fn a082__mm_sha1msg1_epu32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sha") {
            eprintln!("_mm_sha1msg1_epu32: sha unavailable; hardware coverage skipped");
            return;
        }
        intel::_mm_sha1msg1_epu32();
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_sha1msg1_epu32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_sha1msg1_epu32", "none");
}

#[test]
fn a083__mm_sha1msg2_epu32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sha") {
            eprintln!("_mm_sha1msg2_epu32: sha unavailable; hardware coverage skipped");
            return;
        }
        intel::_mm_sha1msg2_epu32();
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_sha1msg2_epu32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_sha1msg2_epu32", "none");
}

#[test]
fn a084__mm_sha1nexte_epu32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sha") {
            eprintln!("_mm_sha1nexte_epu32: sha unavailable; hardware coverage skipped");
            return;
        }
        intel::_mm_sha1nexte_epu32();
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_sha1nexte_epu32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_sha1nexte_epu32", "none");
}

#[test]
fn a085__mm_sha1rnds4_epu32() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, p1, _case, d): (real::__m128i, real::__m128i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm_sha1rnds4_epu32::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sha") {
            eprintln!("_mm_sha1rnds4_epu32: sha unavailable; hardware coverage skipped");
            return;
        }
        intel::_mm_sha1rnds4_epu32();
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            immediate_cases!(compare, (p0, p1, case, &mut d), [0, 1, 2, 3]);
        }
    }
    d.line("_mm_sha1rnds4_epu32", "0,1,2,3");
}

#[test]
fn a086__mm_shuffle_epi32() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m128i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm_shuffle_epi32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_shuffle_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_shuffle_epi32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
                    42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
                    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                    82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
                    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
                    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132,
                    133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148,
                    149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164,
                    165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180,
                    181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196,
                    197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212,
                    213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228,
                    229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244,
                    245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
                ]
            );
        }
    }
    d.line("_mm_shuffle_epi32", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,198,199,200,201,202,203,204,205,206,207,208,209,210,211,212,213,214,215,216,217,218,219,220,221,222,223,224,225,226,227,228,229,230,231,232,233,234,235,236,237,238,239,240,241,242,243,244,245,246,247,248,249,250,251,252,253,254,255");
}

#[test]
fn a087__mm_shuffle_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_shuffle_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("ssse3") {
            eprintln!("_mm_shuffle_epi8: ssse3 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_shuffle_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_shuffle_epi8", "none");
}

#[test]
fn a088__mm_slli_epi32() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m128i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm_slli_epi32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_slli_epi32();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_slli_epi32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
    d.line("_mm_slli_epi32", "0,1,2,7,8,15,16,17,31,32,33,63,64,127,128,255");
}

#[test]
fn a089__mm_srli_si128() {
    let mut d = Digest::new();
    #[inline(never)]
    unsafe fn compare<const N: i32>((p0, _case, d): (real::__m128i, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::_mm_srli_si128::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_srli_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_srli_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
    d.line("_mm_srli_si128", "0,1,2,7,8,15,16,17,31,32,33,63,64,127,128,255");
}

#[test]
fn a090__mm_storeu_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_storeu_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_storeu_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            real::_mm_storeu_si128(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            d.add(&p0.0);
        }
    }
    d.line("_mm_storeu_si128", "none");
}

#[test]
fn a091__mm_sub_epi16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_sub_epi16();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_sub_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_sub_epi16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_sub_epi16", "none");
}

#[test]
fn a092__mm_sub_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_sub_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_sub_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_sub_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_sub_epi8", "none");
}

#[test]
fn a093__mm_unpackhi_epi64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_unpackhi_epi64();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_unpackhi_epi64: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_unpackhi_epi64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_unpackhi_epi64", "none");
}

#[test]
fn a094__mm_unpackhi_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_unpackhi_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_unpackhi_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_unpackhi_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_unpackhi_epi8", "none");
}

#[test]
fn a095__mm_unpacklo_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_unpacklo_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_unpacklo_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_unpacklo_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_unpacklo_epi8", "none");
}

#[test]
fn a096__mm_xor_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_xor_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_xor_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_xor_si128(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_xor_si128", "none");
}

#[test]
fn a097_float_vector_types_layout() {
    let mut d = Digest::new();
    use core::arch::x86_64 as real;
    use core::mem::{align_of, size_of};
    d.add(bytes(&size_of::<real::__m128>()));
    d.add(bytes(&align_of::<real::__m128>()));
    d.add(bytes(&size_of::<real::__m128d>()));
    d.add(bytes(&align_of::<real::__m128d>()));
    d.add(bytes(&size_of::<real::__m256>()));
    d.add(bytes(&align_of::<real::__m256>()));
    d.add(bytes(&size_of::<real::__m256d>()));
    d.add(bytes(&align_of::<real::__m256d>()));
    d.add(bytes(&size_of::<real::__m512>()));
    d.add(bytes(&align_of::<real::__m512>()));
    d.add(bytes(&size_of::<real::__m512d>()));
    d.add(bytes(&align_of::<real::__m512d>()));
    // bit patterns survive a transmute round trip through the real type
    let v = [1.5, -2.0, f32::MIN_POSITIVE, 3.25];
    let r: real::__m128 = unsafe { core::mem::transmute(v) };
    let back: [f32; 4] = unsafe { core::mem::transmute(r) };
    d.add(bytes(&back));
    d.line("float_vector_types_layout", "none");
}

#[test]
fn a098__mm_cmpeq_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cmpeq_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cmpeq_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_cmpeq_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_cmpeq_epi8", "none");
}

#[test]
fn a099__mm_cmpgt_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cmpgt_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cmpgt_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            let expected = real::_mm_cmpgt_epi8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_cmpgt_epi8", "none");
}

#[test]
fn a100__mm_movemask_epi8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_movemask_epi8();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_movemask_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::__m128i = sample(&mut rng, case, 0);
            let expected = real::_mm_movemask_epi8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("_mm_movemask_epi8", "none");
}

#[test]
fn a101__mm_store_si128() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_store_si128();
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_store_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let p1: real::__m128i = sample(&mut rng, case, 1);
            real::_mm_store_si128(p0.0.as_mut_ptr().add((case % 12) * 16).cast(), p1);
            d.add(&p0.0);
        }
    }
    d.line("_mm_store_si128", "none");
}

#[test]
fn a102_sha256_scalar_reference() {
    let mut d = Digest::new();
    if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!("sha") {
        eprintln!("sha256_scalar_reference: sha unavailable");
        return;
    }
    let mut rng = Rng(0x735a2d97e6481bcf);
    unsafe {
        for case in 0..RANDOM_CASES + 17 {
            let a: [u32; 4] = sample(&mut rng, case, 0);
            let b: [u32; 4] = sample(&mut rng, case, 1);
            let c: [u32; 4] = sample(&mut rng, case, 2);
            let got: [u32; 4] = cast(real::_mm_sha256msg1_epu32(cast(a), cast(b)));
            d.add(bytes(&got));
            let got: [u32; 4] = cast(real::_mm_sha256msg2_epu32(cast(a), cast(b)));
            d.add(bytes(&got));
            let got: [u32; 4] = cast(real::_mm_sha256rnds2_epu32(cast(a), cast(b), cast(c)));
            d.add(bytes(&got));
            // Dependent ternary operations, with all possible equal-input combinations.
            for (x, y, z) in [(a, a, c), (a, b, a), (a, b, b), (a, a, a)] {
                let got: [u32; 4] = cast(real::_mm_sha256rnds2_epu32(cast(x), cast(y), cast(z)));

                d.add(bytes(&got));
                let got: [u32; 4] = cast(real::_mm_sha256rnds2_epu32(cast(y), cast(got), cast(z)));
                d.add(bytes(&got));
            }
        }
    }
    d.line("sha256_scalar_reference", "none");
}
