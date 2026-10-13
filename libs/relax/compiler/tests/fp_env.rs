// The floating-point control register as a builtin (x86-64 MXCSR via _mm_getcsr/_mm_setcsr,
// AArch64 FPCR via core::arch::aarch64::{get_fpcr, set_fpcr} under Relax): round trips, and
// flush-to-zero / denormals-are-zero change results (subnormal products become 0) inside the
// region they are set in and only there. Output = rustc --test.
use std::hint::black_box;

#[cfg(target_arch = "x86_64")]
mod env {
    #[allow(deprecated)]
    pub fn get() -> u64 {
        unsafe { core::arch::x86_64::_mm_getcsr() as u64 }
    }
    #[allow(deprecated)]
    pub fn set(x: u64) {
        unsafe { core::arch::x86_64::_mm_setcsr(x as u32) }
    }
    /// FTZ (bit 15) + DAZ (bit 6)
    pub const FLUSH: u64 = 0x8040;
}

#[cfg(target_arch = "aarch64")]
mod env {
    #[cfg(relax)]
    pub fn get() -> u64 {
        unsafe { core::arch::aarch64::get_fpcr() }
    }
    #[cfg(relax)]
    pub fn set(x: u64) {
        unsafe { core::arch::aarch64::set_fpcr(x) }
    }
    #[cfg(not(relax))]
    pub fn get() -> u64 {
        let r: u64;
        unsafe { std::arch::asm!("mrs {0}, fpcr", out(reg) r) };
        r
    }
    #[cfg(not(relax))]
    pub fn set(x: u64) {
        unsafe { std::arch::asm!("msr fpcr, {0}", in(reg) x) };
    }
    /// FZ (bit 24)
    pub const FLUSH: u64 = 1 << 24;
}

#[inline(never)]
fn products(a: &[f32], k: f32, out: &mut [f32]) {
    for i in 0..a.len() {
        out[i] = a[i] * k;
    }
}

#[inline(never)]
fn sum64(a: &[f64], k: f64) -> f64 {
    let mut s = 0.0;
    for x in a {
        s += x * k;
    }
    s
}

#[test]
fn fp_env() {
    let base = env::get();
    let a: Vec<f32> = (0..64).map(|i| f32::MIN_POSITIVE * (1.0 + i as f32 / 64.0)).collect();
    let d: Vec<f64> = (0..64).map(|i| f64::MIN_POSITIVE * (1.0 + i as f64 / 64.0)).collect();
    let mut h = 0u64;
    for round in 0..300 {
        let mut out = vec![0f32; a.len()];
        products(black_box(&a), black_box(0.25), &mut out);
        let s0 = sum64(black_box(&d), black_box(0.125));
        h = h.wrapping_mul(31).wrapping_add(out.iter().map(|x| x.to_bits() as u64).sum::<u64>() ^ s0.to_bits());
        env::set(base | env::FLUSH);
        let on = env::get();
        let mut out2 = vec![0f32; a.len()];
        products(black_box(&a), black_box(0.25), &mut out2);
        let s1 = sum64(black_box(&d), black_box(0.125));
        black_box(&out2);
        black_box(s1);
        env::set(base);
        let off = env::get();
        h = h.wrapping_mul(31).wrapping_add(out2.iter().map(|x| x.to_bits() as u64).sum::<u64>() ^ s1.to_bits());
        if round == 0 {
            println!("| set {} restored {} flushed {}", on & env::FLUSH == env::FLUSH, off == base, out2.iter().all(|x| *x == 0.0) && s1 == 0.0);
        }
    }
    println!("| {:016x}", h);
}
