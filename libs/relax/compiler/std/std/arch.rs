//! std::arch: core::arch plus run-time CPU feature detection (std_detect's
//! `is_x86_feature_detected!` / `is_aarch64_feature_detected!`). Each macro arm is
//! `cfg!(target_feature = ..) || detect::check(Feature::..)` as in std_detect; the run-time
//! check is answered once per process: CPUID/XGETBV on x86_64, sysctl on macOS
//! aarch64, getauxval on Linux aarch64, and processor-feature queries on Windows aarch64.
//! aarch64 lists std_detect's single-word features; x86 only the ones the supported crate graph asks for. Any other literal is a compile error,
//! as an unknown feature is in std.

pub use core::arch::aarch64;
pub use core::arch::x86;
pub use core::arch::x86_64;

pub use crate::is_aarch64_feature_detected;
pub use crate::is_x86_feature_detected;

pub mod detect {
    use core::sync::atomic::{AtomicU64, Ordering};

    /// Feature bits (one namespace for both architectures).
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    #[repr(u8)]
    pub enum Feature {
        // aarch64
        Asimd = 0,
        Fp = 1,
        Crc = 2,
        Aes = 3,
        Pmull = 4,
        Sha2 = 5,
        Sha3 = 6,
        Fp16 = 7,
        Fhm = 8,
        Dotprod = 9,
        Lse = 10,
        Rdm = 11,
        I8mm = 12,
        Bf16 = 13,
        // x86_64
        Sse = 20,
        Sse2 = 21,
        Sse3 = 22,
        Ssse3 = 23,
        Sse41 = 24,
        Sse42 = 25,
        Popcnt = 26,
        Avx = 27,
        Avx2 = 28,
        Fma = 29,
        F16c = 30,
        Bmi1 = 31,
        Bmi2 = 32,
        Lzcnt = 33,
        Pclmulqdq = 34,
        Sha = 35,
        Ermsb = 36,
        Avx512f = 37,
        Avx512bw = 38,
        Avx512bf16 = 39,
        Avx512fp16 = 40,
        Aesni = 41,
        Avx512vl = 42,
        Avx512dq = 43,
        Avx512cd = 44,
        Vpclmulqdq = 45,
        Cmpxchg16b = 46,
        Movbe = 47,
        Xsave = 48,
        Fxsr = 49,
        Adx = 50,
        Rdrand = 51,
        Rdseed = 52,
        Vaes = 53,
        Gfni = 54,
        Avx512ifma = 55,
        Avx512vbmi = 56,
        Avx512vbmi2 = 57,
        Avx512vnni = 58,
        Avx512bitalg = 59,
        Avx512vpopcntdq = 60,
        Avxvnni = 61,
        // aarch64, the rest of std_detect's list (bits 64 on)
        Bti = 64,
        Cssc = 65,
        Dit = 66,
        Dpb = 67,
        Dpb2 = 68,
        Ecv = 69,
        F32mm = 70,
        F64mm = 71,
        Faminmax = 72,
        Fcma = 73,
        Flagm = 74,
        Flagm2 = 75,
        Fp8 = 76,
        Fp8dot2 = 77,
        Fp8dot4 = 78,
        Fp8fma = 79,
        Fpmr = 80,
        Frintts = 81,
        Hbc = 82,
        Jsconv = 83,
        Lse128 = 84,
        Lse2 = 85,
        Lut = 86,
        Mops = 87,
        Mte = 88,
        Paca = 89,
        Pacg = 90,
        Rand = 91,
        Rcpc = 92,
        Rcpc2 = 93,
        Rcpc3 = 94,
        Sb = 95,
        Sm4 = 96,
        Sme = 97,
        Sme2 = 98,
        Sme2p1 = 99,
        Ssbs = 100,
        Sve = 101,
        Sve2 = 102,
        Sve2p1 = 103,
        Tme = 104,
        Wfxt = 105,
    }

    /// The feature bits, low and high words; bit 127 = initialised (written last, so a reader
    /// that sees it sees the low word too).
    static CACHE: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
    const INIT: u128 = 1 << 127;

    pub fn check(f: Feature) -> bool {
        let mut v = ((CACHE[1].load(Ordering::Acquire) as u128) << 64) | CACHE[0].load(Ordering::Relaxed) as u128;
        if v & INIT == 0 {
            v = detect_features() | INIT;
            CACHE[0].store(v as u64, Ordering::Relaxed);
            CACHE[1].store((v >> 64) as u64, Ordering::Release);
        }
        v & (1u128 << (f as u8)) != 0
    }

    fn bit(f: Feature, on: bool) -> u128 {
        if on {
            1u128 << (f as u8)
        } else {
            0
        }
    }

    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    fn sysctl(name: &[u8]) -> bool {
        // NUL-terminated byte strings (Relax has no c".." literals yet)
        extern "C" {
            fn sysctlbyname(name: *const core::ffi::c_char, oldp: *mut core::ffi::c_void, oldlenp: *mut usize, newp: *mut core::ffi::c_void, newlen: usize) -> i32;
        }
        let mut enabled: i32 = 0;
        let mut len: usize = 4;
        let ret = unsafe { sysctlbyname(name.as_ptr() as *const core::ffi::c_char, &mut enabled as *mut i32 as *mut core::ffi::c_void, &mut len, core::ptr::null_mut(), 0) };
        ret == 0 && enabled != 0
    }

    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    fn detect_features() -> u128 {
        // std_detect os/darwin/aarch64.rs (same names and compositions)
        let fp = sysctl(b"hw.optional.floatingpoint\0");
        // std_detect reads hw.optional.AdvSIMD; current macOS names it hw.optional.arm.AdvSIMD / hw.optional.neon
        let asimd = sysctl(b"hw.optional.AdvSIMD\0") || sysctl(b"hw.optional.arm.AdvSIMD\0") || sysctl(b"hw.optional.neon\0");
        let crc_old = sysctl(b"hw.optional.armv8_crc32\0");
        let aes = sysctl(b"hw.optional.arm.FEAT_AES\0");
        let crc = sysctl(b"hw.optional.arm.FEAT_CRC32\0");
        let dotprod = sysctl(b"hw.optional.arm.FEAT_DotProd\0");
        let fhm = sysctl(b"hw.optional.arm.FEAT_FHM\0");
        let fp16 = sysctl(b"hw.optional.arm.FEAT_FP16\0");
        let lse = sysctl(b"hw.optional.arm.FEAT_LSE\0");
        let pmull = sysctl(b"hw.optional.arm.FEAT_PMULL\0");
        let rdm = sysctl(b"hw.optional.arm.FEAT_RDM\0");
        let sha1 = sysctl(b"hw.optional.arm.FEAT_SHA1\0");
        let sha256 = sysctl(b"hw.optional.arm.FEAT_SHA256\0");
        let sha3 = sysctl(b"hw.optional.arm.FEAT_SHA3\0");
        let sha512 = sysctl(b"hw.optional.arm.FEAT_SHA512\0");
        bit(Feature::Fp, fp)
            | bit(Feature::Asimd, asimd)
            | bit(Feature::Crc, crc_old || crc)
            | bit(Feature::Aes, aes && pmull)
            | bit(Feature::Pmull, aes && pmull)
            | bit(Feature::Dotprod, dotprod)
            | bit(Feature::Fhm, fhm)
            | bit(Feature::Fp16, fp16)
            | bit(Feature::Lse, lse)
            | bit(Feature::Rdm, rdm)
            | bit(Feature::Sha2, sha1 && sha256 && asimd)
            | bit(Feature::Sha3, sha512 && sha3 && asimd)
            | bit(Feature::I8mm, sysctl(b"hw.optional.arm.FEAT_I8MM\0"))
            | bit(Feature::Bf16, sysctl(b"hw.optional.arm.FEAT_BF16\0"))
            | bit(Feature::Bti, sysctl(b"hw.optional.arm.FEAT_BTI\0"))
            | bit(Feature::Cssc, sysctl(b"hw.optional.arm.FEAT_CSSC\0"))
            | bit(Feature::Dit, sysctl(b"hw.optional.arm.FEAT_DIT\0"))
            | bit(Feature::Dpb, sysctl(b"hw.optional.arm.FEAT_DPB\0"))
            | bit(Feature::Dpb2, sysctl(b"hw.optional.arm.FEAT_DPB2\0"))
            | bit(Feature::Ecv, sysctl(b"hw.optional.arm.FEAT_ECV\0"))
            | bit(Feature::Fcma, sysctl(b"hw.optional.arm.FEAT_FCMA\0"))
            | bit(Feature::Flagm, sysctl(b"hw.optional.arm.FEAT_FlagM\0"))
            | bit(Feature::Flagm2, sysctl(b"hw.optional.arm.FEAT_FlagM2\0"))
            | bit(Feature::Frintts, sysctl(b"hw.optional.arm.FEAT_FRINTTS\0"))
            | bit(Feature::Hbc, sysctl(b"hw.optional.arm.FEAT_HBC\0"))
            | bit(Feature::Jsconv, sysctl(b"hw.optional.arm.FEAT_JSCVT\0"))
            | bit(Feature::Lse2, sysctl(b"hw.optional.arm.FEAT_LSE2\0"))
            | bit(Feature::Mte, sysctl(b"hw.optional.arm.FEAT_MTE\0") && sysctl(b"hw.optional.arm.FEAT_MTE2\0"))
            | bit(Feature::Paca, sysctl(b"hw.optional.arm.FEAT_PAuth\0"))
            | bit(Feature::Pacg, sysctl(b"hw.optional.arm.FEAT_PAuth\0"))
            | bit(Feature::Rcpc, sysctl(b"hw.optional.arm.FEAT_LRCPC\0"))
            | bit(Feature::Rcpc2, sysctl(b"hw.optional.arm.FEAT_LRCPC2\0"))
            | bit(Feature::Sb, sysctl(b"hw.optional.arm.FEAT_SB\0"))
            | bit(Feature::Sme, sysctl(b"hw.optional.arm.FEAT_SME\0"))
            | bit(Feature::Sme2, sysctl(b"hw.optional.arm.FEAT_SME2\0"))
            | bit(Feature::Sme2p1, sysctl(b"hw.optional.arm.FEAT_SME2p1\0"))
            | bit(Feature::Ssbs, sysctl(b"hw.optional.arm.FEAT_SSBS\0"))
            | bit(Feature::Wfxt, sysctl(b"hw.optional.arm.FEAT_WFxT\0"))
    }

    #[cfg(target_arch = "x86_64")]
    fn detect_features() -> u128 {
        use core::arch::x86_64::{__cpuid, __cpuid_count, _xgetbv, CpuidResult};
        let root = __cpuid(0);
        if root.eax < 1 { return 0; }
        let p = __cpuid(1);
        let zero = CpuidResult { eax: 0, ebx: 0, ecx: 0, edx: 0 };
        let e = if root.eax >= 7 { __cpuid_count(7, 0) } else { zero };
        let e1 = if root.eax >= 7 && e.eax >= 1 { __cpuid_count(7, 1) } else { zero };
        let ext = if __cpuid(0x80000000).eax >= 0x80000001 { __cpuid(0x80000001).ecx } else { 0 };
        // Never execute XGETBV until both CPU XSAVE and OSXSAVE are set.
        let xcr0 = if p.ecx & (3 << 26) == (3 << 26) { unsafe { _xgetbv(0) } } else { 0 };
        let avx_state = xcr0 & 6 == 6;
        let avx = avx_state && p.ecx & (1 << 28) != 0;
        let fma = avx && p.ecx & (1 << 12) != 0;
        let f16c = avx && p.ecx & (1 << 29) != 0;
        // Rust's AVX-512 target feature also implies FMA and F16C.
        //
        // Relax's asm! / global_asm! assembler (backend/src/asm/x64.rs) has no EVEX encoding, so
        // an AVX-512 kernel that names zmm registers fails when it is assembled at run time. Crates
        // that choose kernels by `is_x86_feature_detected!` would choose exactly those, so the
        // AVX-512 features report what code run here can execute, not what the CPU has: none
        // until the assembler encodes EVEX (relax PR #114).
        const ASM_ENCODES_EVEX: bool = false;
        let avx512 = ASM_ENCODES_EVEX && avx && xcr0 & 0xe0 == 0xe0 && fma && f16c;
        // Intel SKL052: affected non-AVX chips falsely advertise BMI1/BMI2.
        let intel = root.ebx == 0x756e6547 && root.edx == 0x49656e69 && root.ecx == 0x6c65746e;
        let bmi = !intel || avx;
        bit(Feature::Sse, p.edx & (1 << 25) != 0)
            | bit(Feature::Sse2, p.edx & (1 << 26) != 0)
            | bit(Feature::Sse3, p.ecx & 1 != 0)
            | bit(Feature::Ssse3, p.ecx & (1 << 9) != 0)
            | bit(Feature::Sse41, p.ecx & (1 << 19) != 0)
            | bit(Feature::Sse42, p.ecx & (1 << 20) != 0)
            | bit(Feature::Popcnt, p.ecx & (1 << 23) != 0)
            | bit(Feature::Pclmulqdq, p.ecx & (1 << 1) != 0)
            | bit(Feature::Aesni, p.ecx & (1 << 25) != 0)
            | bit(Feature::Avx, avx)
            | bit(Feature::Avx2, avx && e.ebx & (1 << 5) != 0)
            | bit(Feature::Fma, fma)
            | bit(Feature::F16c, f16c)
            | bit(Feature::Bmi1, bmi && e.ebx & (1 << 3) != 0)
            | bit(Feature::Bmi2, bmi && e.ebx & (1 << 8) != 0)
            | bit(Feature::Lzcnt, ext & (1 << 5) != 0)
            | bit(Feature::Sha, e.ebx & (1 << 29) != 0)
            | bit(Feature::Ermsb, e.ebx & (1 << 9) != 0)
            | bit(Feature::Avx512f, avx512 && e.ebx & (1 << 16) != 0)
            | bit(Feature::Avx512bw, avx512 && e.ebx & (1 << 30) != 0)
            | bit(Feature::Avx512vl, avx512 && e.ebx & (1 << 31) != 0)
            | bit(Feature::Avx512dq, avx512 && e.ebx & (1 << 17) != 0)
            | bit(Feature::Avx512cd, avx512 && e.ebx & (1 << 28) != 0)
            | bit(Feature::Avx512bf16, avx512 && e1.eax & (1 << 5) != 0)
            | bit(Feature::Avx512fp16, avx512 && e.edx & (1 << 23) != 0)
            | bit(Feature::Vpclmulqdq, avx && e.ecx & (1 << 10) != 0)
            | bit(Feature::Cmpxchg16b, p.ecx & (1 << 13) != 0)
            | bit(Feature::Movbe, p.ecx & (1 << 22) != 0)
            | bit(Feature::Xsave, avx_state && p.ecx & (1 << 26) != 0)
            | bit(Feature::Fxsr, p.edx & (1 << 24) != 0)
            | bit(Feature::Adx, e.ebx & (1 << 19) != 0)
            | bit(Feature::Rdrand, p.ecx & (1 << 30) != 0)
            | bit(Feature::Rdseed, e.ebx & (1 << 18) != 0)
            // std_detect x86.rs: leaf 7 ecx / ebx and leaf 7.1 eax (chacha20 / aes / curve25519 backends)
            | bit(Feature::Vaes, avx && e.ecx & (1 << 9) != 0)
            | bit(Feature::Gfni, e.ecx & (1 << 8) != 0)
            | bit(Feature::Avx512ifma, avx512 && e.ebx & (1 << 21) != 0)
            | bit(Feature::Avx512vbmi, avx512 && e.ecx & (1 << 1) != 0)
            | bit(Feature::Avx512vbmi2, avx512 && e.ecx & (1 << 6) != 0)
            | bit(Feature::Avx512vnni, avx512 && e.ecx & (1 << 11) != 0)
            | bit(Feature::Avx512bitalg, avx512 && e.ecx & (1 << 12) != 0)
            | bit(Feature::Avx512vpopcntdq, avx512 && e.ecx & (1 << 14) != 0)
            | bit(Feature::Avxvnni, avx && e1.eax & (1 << 4) != 0)
    }

    #[cfg(all(any(target_os = "linux", target_os = "android"), target_arch = "aarch64"))]
    fn detect_features() -> u128 {
        extern "C" { fn getauxval(kind: usize) -> usize; }
        let hw = unsafe { getauxval(16) }; // AT_HWCAP
        let hw2 = unsafe { getauxval(26) }; // AT_HWCAP2
        let fp = hw & 1 != 0;
        let fphp = hw & (1 << 9) != 0;
        let asimd = fp && hw & (1 << 1) != 0 && (!fphp || hw & (1 << 10) != 0);
        let sha2 = asimd && hw & (3 << 5) == (3 << 5);
        bit(Feature::Fp, fp)
            | bit(Feature::Asimd, asimd)
            | bit(Feature::Crc, hw & (1 << 7) != 0)
            | bit(Feature::Aes, asimd && hw & (1 << 3) != 0)
            | bit(Feature::Pmull, hw & (1 << 4) != 0)
            | bit(Feature::Sha2, sha2)
            | bit(Feature::Sha3, sha2 && hw & (1 << 17) != 0 && hw & (1 << 21) != 0)
            | bit(Feature::Fp16, fp && fphp)
            | bit(Feature::Fhm, fphp && hw & (1 << 23) != 0)
            | bit(Feature::Dotprod, hw & (1 << 20) != 0)
            | bit(Feature::Lse, hw & (1 << 8) != 0)
            | bit(Feature::Rdm, hw & (1 << 12) != 0)
            | bit(Feature::I8mm, hw2 & (1 << 13) != 0)
            | bit(Feature::Bf16, hw2 & (1 << 14) != 0)
            | detect_more(hw, hw2, fp, asimd)
    }

    /// std_detect os/linux/aarch64.rs: the rest of its single-word features, from the same
    /// AT_HWCAP and AT_HWCAP2 bits and with the same compositions.
    #[cfg(all(any(target_os = "linux", target_os = "android"), target_arch = "aarch64"))]
    fn detect_more(hw: usize, hw2: usize, fp: bool, asimd: bool) -> u128 {
        let h = |n: u32| hw >> n & 1 != 0;
        let h2 = |n: u32| hw2 >> n & 1 != 0;
        let bf16 = h2(14);
        let sve = h(22);
        let rcpc2 = h(26) && h(15);
        let sve2 = h2(1) && sve && asimd;
        let fp8 = h2(51) && h2(50) && h2(49) && bf16;
        let fp8fma = h2(52) && fp8;
        let fp8dot4 = h2(53) && fp8fma;
        let sme = h2(23) && bf16;
        let sme2 = h2(37) && sme;
        bit(Feature::Lse2, h(25))
            | bit(Feature::Lse128, h2(47) && h(8))
            | bit(Feature::Rcpc, h(15))
            | bit(Feature::Rcpc2, rcpc2)
            | bit(Feature::Rcpc3, h2(46) && rcpc2)
            | bit(Feature::Dit, h(24))
            | bit(Feature::Flagm, h(27))
            | bit(Feature::Flagm2, h2(7))
            | bit(Feature::Ssbs, h(28))
            | bit(Feature::Sb, h(29))
            | bit(Feature::Paca, h(30))
            | bit(Feature::Pacg, h(31))
            | bit(Feature::Dpb, h(16))
            | bit(Feature::Dpb2, h2(0))
            | bit(Feature::Rand, h2(16))
            | bit(Feature::Bti, h2(17))
            | bit(Feature::Mte, h2(18))
            | bit(Feature::Jsconv, h(13) && fp)
            | bit(Feature::Frintts, h2(8))
            | bit(Feature::Fcma, h(14) && asimd)
            | bit(Feature::Sve, sve && asimd)
            | bit(Feature::F32mm, h2(10) && sve && asimd)
            | bit(Feature::F64mm, h2(11) && sve && asimd)
            | bit(Feature::Sm4, h(18) && h(19) && asimd)
            | bit(Feature::Sve2, sve2)
            | bit(Feature::Sve2p1, h2(36) && sve2)
            | bit(Feature::Hbc, h2(44))
            | bit(Feature::Mops, h2(43))
            | bit(Feature::Ecv, h2(19))
            | bit(Feature::Lut, h2(49))
            | bit(Feature::Cssc, h2(34))
            | bit(Feature::Fpmr, h2(48))
            | bit(Feature::Faminmax, h2(50))
            | bit(Feature::Fp8, fp8)
            | bit(Feature::Fp8fma, fp8fma)
            | bit(Feature::Fp8dot4, fp8dot4)
            | bit(Feature::Fp8dot2, h2(54) && fp8dot4)
            | bit(Feature::Wfxt, h2(31))
            | bit(Feature::Sme, sme)
            | bit(Feature::Sme2, sme2)
            | bit(Feature::Sme2p1, h2(38) && sme2)
    }

    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    fn detect_features() -> u128 {
        #[link(name = "kernel32")]
        extern "system" { fn IsProcessorFeaturePresent(feature: u32) -> i32; }
        unsafe {
            let crypto = IsProcessorFeaturePresent(30) != 0;
            let dotprod = IsProcessorFeaturePresent(43) != 0;
            bit(Feature::Fp, IsProcessorFeaturePresent(18) != 0)
                | bit(Feature::Asimd, IsProcessorFeaturePresent(19) != 0)
                | bit(Feature::Crc, IsProcessorFeaturePresent(31) != 0)
                | bit(Feature::Lse, IsProcessorFeaturePresent(34) != 0)
                | bit(Feature::Aes, crypto)
                | bit(Feature::Pmull, crypto)
                | bit(Feature::Sha2, crypto)
                | bit(Feature::Dotprod, dotprod)
                // AdvSIMD + v8.2 DotProd imply the v8.1 RDM instructions.
                | bit(Feature::Rdm, dotprod)
                | bit(Feature::Fp16, IsProcessorFeaturePresent(67) != 0)
                | bit(Feature::Sha3, IsProcessorFeaturePresent(64) != 0 && IsProcessorFeaturePresent(65) != 0)
                | bit(Feature::I8mm, IsProcessorFeaturePresent(66) != 0)
                | bit(Feature::Bf16, IsProcessorFeaturePresent(68) != 0)
            // Windows exposes no FHM processor-feature flag.
        }
    }

    #[cfg(not(any(target_arch = "x86_64", all(target_arch = "aarch64", any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "windows")))))]
    fn detect_features() -> u128 { 0 }

}

#[macro_export]
macro_rules! is_aarch64_feature_detected {
    ("neon") => { cfg!(target_feature = "neon") || $crate::arch::detect::check($crate::arch::detect::Feature::Asimd) };
    ("asimd") => { cfg!(target_feature = "neon") || $crate::arch::detect::check($crate::arch::detect::Feature::Asimd) };
    ("fp") => { cfg!(target_feature = "neon") || $crate::arch::detect::check($crate::arch::detect::Feature::Fp) };
    ("crc") => { cfg!(target_feature = "crc") || $crate::arch::detect::check($crate::arch::detect::Feature::Crc) };
    ("aes") => { cfg!(target_feature = "aes") || $crate::arch::detect::check($crate::arch::detect::Feature::Aes) };
    ("pmull") => { cfg!(target_feature = "aes") || $crate::arch::detect::check($crate::arch::detect::Feature::Pmull) };
    ("sha2") => { cfg!(target_feature = "sha2") || $crate::arch::detect::check($crate::arch::detect::Feature::Sha2) };
    ("sha3") => { cfg!(target_feature = "sha3") || $crate::arch::detect::check($crate::arch::detect::Feature::Sha3) };
    ("fp16") => { cfg!(target_feature = "fp16") || $crate::arch::detect::check($crate::arch::detect::Feature::Fp16) };
    ("fhm") => { cfg!(target_feature = "fhm") || $crate::arch::detect::check($crate::arch::detect::Feature::Fhm) };
    ("dotprod") => { cfg!(target_feature = "dotprod") || $crate::arch::detect::check($crate::arch::detect::Feature::Dotprod) };
    ("lse") => { cfg!(target_feature = "lse") || $crate::arch::detect::check($crate::arch::detect::Feature::Lse) };
    ("rdm") => { cfg!(target_feature = "rdm") || $crate::arch::detect::check($crate::arch::detect::Feature::Rdm) };
    ("i8mm") => { cfg!(target_feature = "i8mm") || $crate::arch::detect::check($crate::arch::detect::Feature::I8mm) };
    ("bf16") => { cfg!(target_feature = "bf16") || $crate::arch::detect::check($crate::arch::detect::Feature::Bf16) };
    ("bti") => { cfg!(target_feature = "bti") || $crate::arch::detect::check($crate::arch::detect::Feature::Bti) };
    ("cssc") => { cfg!(target_feature = "cssc") || $crate::arch::detect::check($crate::arch::detect::Feature::Cssc) };
    ("dit") => { cfg!(target_feature = "dit") || $crate::arch::detect::check($crate::arch::detect::Feature::Dit) };
    ("dpb") => { cfg!(target_feature = "dpb") || $crate::arch::detect::check($crate::arch::detect::Feature::Dpb) };
    ("dpb2") => { cfg!(target_feature = "dpb2") || $crate::arch::detect::check($crate::arch::detect::Feature::Dpb2) };
    ("ecv") => { cfg!(target_feature = "ecv") || $crate::arch::detect::check($crate::arch::detect::Feature::Ecv) };
    ("f32mm") => { cfg!(target_feature = "f32mm") || $crate::arch::detect::check($crate::arch::detect::Feature::F32mm) };
    ("f64mm") => { cfg!(target_feature = "f64mm") || $crate::arch::detect::check($crate::arch::detect::Feature::F64mm) };
    ("faminmax") => { cfg!(target_feature = "faminmax") || $crate::arch::detect::check($crate::arch::detect::Feature::Faminmax) };
    ("fcma") => { cfg!(target_feature = "fcma") || $crate::arch::detect::check($crate::arch::detect::Feature::Fcma) };
    ("flagm") => { cfg!(target_feature = "flagm") || $crate::arch::detect::check($crate::arch::detect::Feature::Flagm) };
    ("flagm2") => { cfg!(target_feature = "flagm2") || $crate::arch::detect::check($crate::arch::detect::Feature::Flagm2) };
    ("fp8") => { cfg!(target_feature = "fp8") || $crate::arch::detect::check($crate::arch::detect::Feature::Fp8) };
    ("fp8dot2") => { cfg!(target_feature = "fp8dot2") || $crate::arch::detect::check($crate::arch::detect::Feature::Fp8dot2) };
    ("fp8dot4") => { cfg!(target_feature = "fp8dot4") || $crate::arch::detect::check($crate::arch::detect::Feature::Fp8dot4) };
    ("fp8fma") => { cfg!(target_feature = "fp8fma") || $crate::arch::detect::check($crate::arch::detect::Feature::Fp8fma) };
    ("fpmr") => { cfg!(target_feature = "fpmr") || $crate::arch::detect::check($crate::arch::detect::Feature::Fpmr) };
    ("frintts") => { cfg!(target_feature = "frintts") || $crate::arch::detect::check($crate::arch::detect::Feature::Frintts) };
    ("hbc") => { cfg!(target_feature = "hbc") || $crate::arch::detect::check($crate::arch::detect::Feature::Hbc) };
    ("jsconv") => { cfg!(target_feature = "jsconv") || $crate::arch::detect::check($crate::arch::detect::Feature::Jsconv) };
    ("lse128") => { cfg!(target_feature = "lse128") || $crate::arch::detect::check($crate::arch::detect::Feature::Lse128) };
    ("lse2") => { cfg!(target_feature = "lse2") || $crate::arch::detect::check($crate::arch::detect::Feature::Lse2) };
    ("lut") => { cfg!(target_feature = "lut") || $crate::arch::detect::check($crate::arch::detect::Feature::Lut) };
    ("mops") => { cfg!(target_feature = "mops") || $crate::arch::detect::check($crate::arch::detect::Feature::Mops) };
    ("mte") => { cfg!(target_feature = "mte") || $crate::arch::detect::check($crate::arch::detect::Feature::Mte) };
    ("paca") => { cfg!(target_feature = "paca") || $crate::arch::detect::check($crate::arch::detect::Feature::Paca) };
    ("pacg") => { cfg!(target_feature = "pacg") || $crate::arch::detect::check($crate::arch::detect::Feature::Pacg) };
    ("rand") => { cfg!(target_feature = "rand") || $crate::arch::detect::check($crate::arch::detect::Feature::Rand) };
    ("rcpc") => { cfg!(target_feature = "rcpc") || $crate::arch::detect::check($crate::arch::detect::Feature::Rcpc) };
    ("rcpc2") => { cfg!(target_feature = "rcpc2") || $crate::arch::detect::check($crate::arch::detect::Feature::Rcpc2) };
    ("rcpc3") => { cfg!(target_feature = "rcpc3") || $crate::arch::detect::check($crate::arch::detect::Feature::Rcpc3) };
    ("sb") => { cfg!(target_feature = "sb") || $crate::arch::detect::check($crate::arch::detect::Feature::Sb) };
    ("sm4") => { cfg!(target_feature = "sm4") || $crate::arch::detect::check($crate::arch::detect::Feature::Sm4) };
    ("sme") => { cfg!(target_feature = "sme") || $crate::arch::detect::check($crate::arch::detect::Feature::Sme) };
    ("sme2") => { cfg!(target_feature = "sme2") || $crate::arch::detect::check($crate::arch::detect::Feature::Sme2) };
    ("sme2p1") => { cfg!(target_feature = "sme2p1") || $crate::arch::detect::check($crate::arch::detect::Feature::Sme2p1) };
    ("ssbs") => { cfg!(target_feature = "ssbs") || $crate::arch::detect::check($crate::arch::detect::Feature::Ssbs) };
    ("sve") => { cfg!(target_feature = "sve") || $crate::arch::detect::check($crate::arch::detect::Feature::Sve) };
    ("sve2") => { cfg!(target_feature = "sve2") || $crate::arch::detect::check($crate::arch::detect::Feature::Sve2) };
    ("sve2p1") => { cfg!(target_feature = "sve2p1") || $crate::arch::detect::check($crate::arch::detect::Feature::Sve2p1) };
    ("tme") => { cfg!(target_feature = "tme") || $crate::arch::detect::check($crate::arch::detect::Feature::Tme) };
    ("wfxt") => { cfg!(target_feature = "wfxt") || $crate::arch::detect::check($crate::arch::detect::Feature::Wfxt) };
    ($t:tt,) => { $crate::is_aarch64_feature_detected!($t) };
    ($t:tt) => { compile_error!(concat!("unknown aarch64 target feature: ", $t)) };
}

#[macro_export]
macro_rules! is_x86_feature_detected {
    ("sse") => { cfg!(target_feature = "sse") || $crate::arch::detect::check($crate::arch::detect::Feature::Sse) };
    ("sse2") => { cfg!(target_feature = "sse2") || $crate::arch::detect::check($crate::arch::detect::Feature::Sse2) };
    ("sse3") => { cfg!(target_feature = "sse3") || $crate::arch::detect::check($crate::arch::detect::Feature::Sse3) };
    ("ssse3") => { cfg!(target_feature = "ssse3") || $crate::arch::detect::check($crate::arch::detect::Feature::Ssse3) };
    ("sse4.1") => { cfg!(target_feature = "sse4.1") || $crate::arch::detect::check($crate::arch::detect::Feature::Sse41) };
    ("sse4.2") => { cfg!(target_feature = "sse4.2") || $crate::arch::detect::check($crate::arch::detect::Feature::Sse42) };
    ("popcnt") => { cfg!(target_feature = "popcnt") || $crate::arch::detect::check($crate::arch::detect::Feature::Popcnt) };
    ("avx") => { cfg!(target_feature = "avx") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx) };
    ("avx2") => { cfg!(target_feature = "avx2") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx2) };
    ("fma") => { cfg!(target_feature = "fma") || $crate::arch::detect::check($crate::arch::detect::Feature::Fma) };
    ("f16c") => { cfg!(target_feature = "f16c") || $crate::arch::detect::check($crate::arch::detect::Feature::F16c) };
    ("bmi1") => { cfg!(target_feature = "bmi1") || $crate::arch::detect::check($crate::arch::detect::Feature::Bmi1) };
    ("bmi2") => { cfg!(target_feature = "bmi2") || $crate::arch::detect::check($crate::arch::detect::Feature::Bmi2) };
    ("lzcnt") => { cfg!(target_feature = "lzcnt") || $crate::arch::detect::check($crate::arch::detect::Feature::Lzcnt) };
    ("pclmulqdq") => { cfg!(target_feature = "pclmulqdq") || $crate::arch::detect::check($crate::arch::detect::Feature::Pclmulqdq) };
    ("sha") => { cfg!(target_feature = "sha") || $crate::arch::detect::check($crate::arch::detect::Feature::Sha) };
    ("ermsb") => { cfg!(target_feature = "ermsb") || $crate::arch::detect::check($crate::arch::detect::Feature::Ermsb) };
    ("aes") => { cfg!(target_feature = "aes") || $crate::arch::detect::check($crate::arch::detect::Feature::Aesni) };
    ("avx512f") => { cfg!(target_feature = "avx512f") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512f) };
    ("avx512bw") => { cfg!(target_feature = "avx512bw") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512bw) };
    ("avx512vl") => { cfg!(target_feature = "avx512vl") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512vl) };
    ("avx512dq") => { cfg!(target_feature = "avx512dq") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512dq) };
    ("avx512cd") => { cfg!(target_feature = "avx512cd") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512cd) };
    ("avx512bf16") => { cfg!(target_feature = "avx512bf16") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512bf16) };
    ("avx512fp16") => { cfg!(target_feature = "avx512fp16") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512fp16) };
    ("vpclmulqdq") => { cfg!(target_feature = "vpclmulqdq") || $crate::arch::detect::check($crate::arch::detect::Feature::Vpclmulqdq) };
    ("cmpxchg16b") => { cfg!(target_feature = "cmpxchg16b") || $crate::arch::detect::check($crate::arch::detect::Feature::Cmpxchg16b) };
    ("movbe") => { cfg!(target_feature = "movbe") || $crate::arch::detect::check($crate::arch::detect::Feature::Movbe) };
    ("xsave") => { cfg!(target_feature = "xsave") || $crate::arch::detect::check($crate::arch::detect::Feature::Xsave) };
    ("fxsr") => { cfg!(target_feature = "fxsr") || $crate::arch::detect::check($crate::arch::detect::Feature::Fxsr) };
    ("adx") => { cfg!(target_feature = "adx") || $crate::arch::detect::check($crate::arch::detect::Feature::Adx) };
    ("rdrand") => { cfg!(target_feature = "rdrand") || $crate::arch::detect::check($crate::arch::detect::Feature::Rdrand) };
    ("rdseed") => { cfg!(target_feature = "rdseed") || $crate::arch::detect::check($crate::arch::detect::Feature::Rdseed) };
    ("vaes") => { cfg!(target_feature = "vaes") || $crate::arch::detect::check($crate::arch::detect::Feature::Vaes) };
    ("gfni") => { cfg!(target_feature = "gfni") || $crate::arch::detect::check($crate::arch::detect::Feature::Gfni) };
    ("avx512ifma") => { cfg!(target_feature = "avx512ifma") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512ifma) };
    ("avx512vbmi") => { cfg!(target_feature = "avx512vbmi") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512vbmi) };
    ("avx512vbmi2") => { cfg!(target_feature = "avx512vbmi2") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512vbmi2) };
    ("avx512vnni") => { cfg!(target_feature = "avx512vnni") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512vnni) };
    ("avx512bitalg") => { cfg!(target_feature = "avx512bitalg") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512bitalg) };
    ("avx512vpopcntdq") => { cfg!(target_feature = "avx512vpopcntdq") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512vpopcntdq) };
    ("avxvnni") => { cfg!(target_feature = "avxvnni") || $crate::arch::detect::check($crate::arch::detect::Feature::Avxvnni) };
    ($t:tt,) => { $crate::is_x86_feature_detected!($t) };
    ($t:tt) => { compile_error!(concat!("unknown x86 target feature: ", $t)) };
}
