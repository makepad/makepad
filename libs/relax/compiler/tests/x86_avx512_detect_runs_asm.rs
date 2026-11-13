// What `is_x86_feature_detected!` reports runs: a program that picks an AVX-512 kernel written with
// `asm!` naming zmm registers when "avx512f" is detected (as pulp does) runs that kernel or the other
// one, never one Relax cannot assemble (relax PR #114: Relax's assembler encodes no EVEX, so it reports
// no AVX-512 features).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[inline(never)]
unsafe fn copy64(src: &[u32; 16], dst: &mut [u32; 16]) {
    core::arch::asm!(
        "vmovdqu64 zmm0, [{src}]",
        "vmovdqu64 [{dst}], zmm0",
        src = in(reg) src.as_ptr(),
        dst = in(reg) dst.as_mut_ptr(),
        out("zmm0") _,
        options(nostack),
    );
}

/// The AVX-512 kernel when "avx512f" is detected, else a plain copy.
fn copy(src: &[u32; 16]) -> [u32; 16] {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("avx512f") {
        let mut dst = [0u32; 16];
        unsafe { copy64(src, &mut dst) };
        return dst;
    }
    *src
}

#[test]
fn the_detected_kernel_runs() {
    let src: [u32; 16] = core::array::from_fn(|i| i as u32 * 3);
    assert_eq!(copy(&src), src);
}
