//! std::arch: core::arch plus run-time CPU feature detection (std_detect's
//! `is_x86_feature_detected!` / `is_aarch64_feature_detected!`). Each macro arm is
//! `cfg!(target_feature = ..) || detect::check(Feature::..)` as in std_detect; the run-time
//! check is answered once per process: CPUID/XGETBV on x86_64, sysctl on macOS
//! aarch64, getauxval on Linux aarch64, and processor-feature queries on Windows aarch64.
//! Only the features the supported crate graph asks for are listed; any other literal is a compile error,
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
    }

    /// bit 63 = initialised
    static CACHE: AtomicU64 = AtomicU64::new(0);
    const INIT: u64 = 1 << 63;

    pub fn check(f: Feature) -> bool {
        let mut v = CACHE.load(Ordering::Relaxed);
        if v & INIT == 0 {
            v = detect_features() | INIT;
            CACHE.store(v, Ordering::Relaxed);
        }
        v & (1u64 << (f as u8)) != 0
    }

    fn bit(f: Feature, on: bool) -> u64 {
        if on {
            1u64 << (f as u8)
        } else {
            0
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
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

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn detect_features() -> u64 {
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
    }

    #[cfg(target_arch = "x86_64")]
    fn detect_features() -> u64 {
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
        let avx512 = avx && xcr0 & 0xe0 == 0xe0 && fma && f16c;
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
            | bit(Feature::Avx512bf16, avx512 && e1.eax & (1 << 5) != 0)
            | bit(Feature::Avx512fp16, avx512 && e.edx & (1 << 23) != 0)
    }

    #[cfg(all(any(target_os = "linux", target_os = "android"), target_arch = "aarch64"))]
    fn detect_features() -> u64 {
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
    }

    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    fn detect_features() -> u64 {
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

    #[cfg(not(any(target_arch = "x86_64", all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux", target_os = "android", target_os = "windows")))))]
    fn detect_features() -> u64 { 0 }

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
    ("avx512bf16") => { cfg!(target_feature = "avx512bf16") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512bf16) };
    ("avx512fp16") => { cfg!(target_feature = "avx512fp16") || $crate::arch::detect::check($crate::arch::detect::Feature::Avx512fp16) };
    ($t:tt,) => { $crate::is_x86_feature_detected!($t) };
    ($t:tt) => { compile_error!(concat!("unknown x86 target feature: ", $t)) };
}
