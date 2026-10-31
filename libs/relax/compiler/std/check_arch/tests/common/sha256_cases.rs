// Shared by check_arch and compiler/tests/sha256_intrinsics.rs.
// The includer declares `mod sha256_oracle` (include! of common/sha256_oracle.rs, path relative to the includer).

#[cfg(target_arch = "x86_64")]
#[test]
fn sha256_scalar_reference() {
    assert!(std::is_x86_feature_detected!("sha"), "SHA-NI coverage requires a SHA-capable host");
    let mut rng = Rng(0x735a2d97e6481bcf);
    unsafe {
        for case in 0..RANDOM_CASES + 17 {
            let a: [u32; 4] = sample(&mut rng, case, 0);
            let b: [u32; 4] = sample(&mut rng, case, 1);
            let c: [u32; 4] = sample(&mut rng, case, 2);
            let got: [u32; 4] = cast(hr::_mm_sha256msg1_epu32(cast(a), cast(b)));
            assert_eq!(got, sha256_oracle::_mm_sha256msg1_epu32(a, b), "msg1 case {case}");
            let got: [u32; 4] = cast(hr::_mm_sha256msg2_epu32(cast(a), cast(b)));
            assert_eq!(got, sha256_oracle::_mm_sha256msg2_epu32(a, b), "msg2 case {case}");
            let got: [u32; 4] = cast(hr::_mm_sha256rnds2_epu32(cast(a), cast(b), cast(c)));
            assert_eq!(got, sha256_oracle::_mm_sha256rnds2_epu32(a, b, c), "rnds2 case {case}");
            // Dependent ternary operations, with all possible equal-input combinations.
            for (x, y, z) in [(a, a, c), (a, b, a), (a, b, b), (a, a, a)] {
                let got: [u32; 4] = cast(hr::_mm_sha256rnds2_epu32(cast(x), cast(y), cast(z)));
                let expected = sha256_oracle::_mm_sha256rnds2_epu32(x, y, z);
                assert_eq!(got, expected, "rnds2 aliases case {case}");
                let got: [u32; 4] = cast(hr::_mm_sha256rnds2_epu32(cast(y), cast(got), cast(z)));
                assert_eq!(got, sha256_oracle::_mm_sha256rnds2_epu32(y, expected, z), "rnds2 chain case {case}");
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[test]
fn sha256_scalar_reference() {
    assert!(std::arch::is_aarch64_feature_detected!("sha2"), "SHA256 coverage requires sha2");
    let mut rng = Rng(0x735a2d97e6481bcf);
    unsafe {
        for case in 0..RANDOM_CASES + 17 {
            let a: [u32; 4] = sample(&mut rng, case, 0);
            let b: [u32; 4] = sample(&mut rng, case, 1);
            let c: [u32; 4] = sample(&mut rng, case, 2);
            let got: [u32; 4] = cast(hr::vsha256su0q_u32(cast(a), cast(b)));
            assert_eq!(got, sha256_oracle::vsha256su0q_u32(a, b), "su0 case {case}");
            for (x, y, z) in [(a, b, c), (a, a, c), (a, b, a), (a, b, b), (a, a, a)] {
                let got: [u32; 4] = cast(hr::vsha256hq_u32(cast(x), cast(y), cast(z)));
                assert_eq!(got, sha256_oracle::vsha256hq_u32(x, y, z), "h case {case}");
                let got: [u32; 4] = cast(hr::vsha256h2q_u32(cast(x), cast(y), cast(z)));
                assert_eq!(got, sha256_oracle::vsha256h2q_u32(x, y, z), "h2 case {case}");
                let got: [u32; 4] = cast(hr::vsha256su1q_u32(cast(x), cast(y), cast(z)));
                assert_eq!(got, sha256_oracle::vsha256su1q_u32(x, y, z), "su1 case {case}");
            }
        }
    }
}

#[cfg(all(target_arch = "aarch64", relax))]
#[test]
fn intel_sha256_on_arm() {
    use core::arch::x86_64 as x86;
    let mut rng = Rng(0x735a2d97e6481bcf);
    unsafe {
        for case in 0..RANDOM_CASES + 17 {
            let a: [u32; 4] = sample(&mut rng, case, 0);
            let b: [u32; 4] = sample(&mut rng, case, 1);
            let c: [u32; 4] = sample(&mut rng, case, 2);
            let got: [u32; 4] = cast(x86::_mm_sha256msg1_epu32(cast(a), cast(b)));
            assert_eq!(got, sha256_oracle::_mm_sha256msg1_epu32(a, b), "msg1 case {case}");
            let got: [u32; 4] = cast(x86::_mm_sha256msg2_epu32(cast(a), cast(b)));
            assert_eq!(got, sha256_oracle::_mm_sha256msg2_epu32(a, b), "msg2 case {case}");
            let got: [u32; 4] = cast(x86::_mm_sha256rnds2_epu32(cast(a), cast(b), cast(c)));
            assert_eq!(got, sha256_oracle::_mm_sha256rnds2_epu32(a, b, c), "rnds2 case {case}");
        }
    }
}
