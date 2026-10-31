// core::arch::aarch64 SHA-256 intrinsics (vsha256hq/h2q/su0q/su1q_u32) on pseudo-random inputs,
// plus a full block compressed with them. "| " lines must equal real std's (core_diff.sh, arm64).
#[cfg(target_arch = "aarch64")]
use core::arch::aarch64::*;

#[cfg(target_arch = "aarch64")]
fn v(x: &mut u64) -> uint32x4_t {
    let mut a = [0u32; 4];
    for i in 0..4 {
        *x ^= *x << 13;
        *x ^= *x >> 7;
        *x ^= *x << 17;
        a[i] = *x as u32;
    }
    unsafe { vld1q_u32(a.as_ptr()) }
}

#[cfg(target_arch = "aarch64")]
fn out(r: uint32x4_t) -> [u32; 4] {
    let mut a = [0u32; 4];
    unsafe { vst1q_u32(a.as_mut_ptr(), r) };
    a
}

#[cfg(target_arch = "aarch64")]
#[test]
fn a01_sha256_intrinsics() {
    let mut s = 0x9e3779b97f4a7c15u64;
    for round in 0..4 {
        let (a, b, c) = (v(&mut s), v(&mut s), v(&mut s));
        unsafe {
            println!("| {} h {:08x?}", round, out(vsha256hq_u32(a, b, c)));
            println!("| {} h2 {:08x?}", round, out(vsha256h2q_u32(a, b, c)));
            println!("| {} su0 {:08x?}", round, out(vsha256su0q_u32(a, b)));
            println!("| {} su1 {:08x?}", round, out(vsha256su1q_u32(a, b, c)));
        }
    }
}
