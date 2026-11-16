// diff_fast corpus program: is_x86_feature_detected! for every feature name stable std accepts, printed
// as `D <feature> <bool>` so the CPUID / XGETBV bit for each must match std_detect's on this machine.
// The AVX-512 names only have to compile: Relax reports no AVX-512 (its run-time assembler has no EVEX
// encoding), so their values differ from rustc's by design. Other hosts print one line.

macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }

#[test]
fn run() {
    work();
}

#[cfg(not(target_arch = "x86_64"))]
fn work() {
    d!("x86 detection: x86-64 only");
}

#[cfg(target_arch = "x86_64")]
fn work() {
    macro_rules! show {
        // tt, not literal: a captured `literal` fragment is opaque and matches no literal arm
        ($($f:tt)*) => {$( d!("{} {}", $f, std::is_x86_feature_detected!($f)); )*};
    }
    show!("aes" "pclmulqdq" "rdrand" "rdseed" "tsc" "mmx" "sse" "sse2" "sse3" "ssse3" "sse4.1" "sse4.2"
        "sse4a" "sha" "avx" "avx2" "sha512" "sm3" "sm4" "avxifma" "avxneconvert" "avxvnni" "avxvnniint16"
        "avxvnniint8" "f16c" "fma" "bmi1" "bmi2" "lzcnt" "tbm" "popcnt" "fxsr" "xsave" "xsaveopt" "xsaves"
        "xsavec" "cmpxchg16b" "kl" "widekl" "adx" "rtm" "movbe" "ermsb" "gfni" "vaes" "vpclmulqdq");
    let avx512 = [
        std::is_x86_feature_detected!("avx512f"),
        std::is_x86_feature_detected!("avx512cd"),
        std::is_x86_feature_detected!("avx512er"),
        std::is_x86_feature_detected!("avx512pf"),
        std::is_x86_feature_detected!("avx512bw"),
        std::is_x86_feature_detected!("avx512dq"),
        std::is_x86_feature_detected!("avx512vl"),
        std::is_x86_feature_detected!("avx512ifma"),
        std::is_x86_feature_detected!("avx512vbmi"),
        std::is_x86_feature_detected!("avx512vpopcntdq"),
        std::is_x86_feature_detected!("avx512vbmi2"),
        std::is_x86_feature_detected!("avx512vnni"),
        std::is_x86_feature_detected!("avx512bitalg"),
        std::is_x86_feature_detected!("avx512bf16"),
        std::is_x86_feature_detected!("avx512vp2intersect"),
        std::is_x86_feature_detected!("avx512fp16"),
    ];
    // a sub-feature never reports without avx512f
    d!("avx512 consistent {}", avx512[0] || avx512.iter().all(|&b| !b));
}
