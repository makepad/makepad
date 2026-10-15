//! Old inline assembly vs the code that replaced it, bit for bit, on special values and a few
//! million pseudo-random inputs (aarch64: libm fma/rint/sqrt, half conversions and f16
//! arithmetic, whisper's f16x4 widening; x86_64: libm sqrt and fma).
#[allow(unused)]
fn rng(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

#[allow(unused)]
fn f64s(n: usize) -> Vec<f64> {
    let mut v = vec![0.0, -0.0, 1.0, -1.0, 0.5, 1.5, 2.5, -2.5, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, f64::MIN_POSITIVE, 5e-324, f64::MAX, 4503599627370495.5];
    let mut s = 0x9e3779b97f4a7c15u64;
    for _ in 0..n {
        v.push(f64::from_bits(rng(&mut s)));
        v.push((rng(&mut s) as i64 as f64) / 1e9);
    }
    v
}

#[allow(unused)]
fn same64(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}
#[allow(unused)]
fn same32(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

#[cfg(target_arch = "aarch64")]
mod old {
    use core::arch::asm;
    pub fn fma(mut x: f64, y: f64, z: f64) -> f64 {
        unsafe { asm!("fmadd {x:d}, {x:d}, {y:d}, {z:d}", x = inout(vreg) x, y = in(vreg) y, z = in(vreg) z, options(nomem, nostack, pure)) };
        x
    }
    pub fn fmaf(mut x: f32, y: f32, z: f32) -> f32 {
        unsafe { asm!("fmadd {x:s}, {x:s}, {y:s}, {z:s}", x = inout(vreg) x, y = in(vreg) y, z = in(vreg) z, options(nomem, nostack, pure)) };
        x
    }
    pub fn rint(mut x: f64) -> f64 {
        unsafe { asm!("frintn {x:d}, {x:d}", x = inout(vreg) x, options(nomem, nostack, pure)) };
        x
    }
    pub fn rintf(mut x: f32) -> f32 {
        unsafe { asm!("frintn {x:s}, {x:s}", x = inout(vreg) x, options(nomem, nostack, pure)) };
        x
    }
    pub fn sqrt(mut x: f64) -> f64 {
        unsafe { asm!("fsqrt {x:d}, {x:d}", x = inout(vreg) x, options(nomem, nostack, pure)) };
        x
    }
    pub fn sqrtf(mut x: f32) -> f32 {
        unsafe { asm!("fsqrt {x:s}, {x:s}", x = inout(vreg) x, options(nomem, nostack, pure)) };
        x
    }
    pub fn f16_to_f32(i: u16) -> f32 {
        let r: f32;
        unsafe { asm!("fcvt {0:s}, {1:h}", out(vreg) r, in(vreg) i, options(pure, nomem, nostack)) };
        r
    }
    pub fn f32_to_f16(f: f32) -> u16 {
        let r: u16;
        unsafe { asm!("fcvt {0:h}, {1:s}", out(vreg) r, in(vreg) f, options(pure, nomem, nostack)) };
        r
    }
    pub fn f64_to_f16(f: f64) -> u16 {
        let r: u16;
        unsafe { asm!("fcvt {0:h}, {1:d}", out(vreg) r, in(vreg) f, options(pure, nomem, nostack)) };
        r
    }
    pub fn f16_op(op: u8, a: u16, b: u16) -> u16 {
        let r: u16;
        unsafe {
            match op {
                0 => asm!("fadd {0:h}, {1:h}, {2:h}", out(vreg) r, in(vreg) a, in(vreg) b, options(pure, nomem, nostack)),
                1 => asm!("fsub {0:h}, {1:h}, {2:h}", out(vreg) r, in(vreg) a, in(vreg) b, options(pure, nomem, nostack)),
                2 => asm!("fmul {0:h}, {1:h}, {2:h}", out(vreg) r, in(vreg) a, in(vreg) b, options(pure, nomem, nostack)),
                _ => asm!("fdiv {0:h}, {1:h}, {2:h}", out(vreg) r, in(vreg) a, in(vreg) b, options(pure, nomem, nostack)),
            }
        }
        r
    }
    pub fn fcvtl(h: [u16; 4]) -> [f32; 4] {
        use core::arch::aarch64::*;
        unsafe {
            let v = vld1_u16(h.as_ptr());
            let out: float32x4_t;
            asm!("fcvtl {out:v}.4s, {inp:v}.4h", inp = in(vreg) v, out = out(vreg) out, options(pure, nomem, nostack));
            let mut r = [0f32; 4];
            vst1q_f32(r.as_mut_ptr(), out);
            r
        }
    }
}

/// whisper tensor.rs's replacement, copied
#[cfg(target_arch = "aarch64")]
fn new_fcvtl(h: [u16; 4]) -> [f32; 4] {
    use core::arch::aarch64::*;
    unsafe {
        let h = vld1_u16(h.as_ptr());
        let w = vmovl_u16(h);
        let sign = vshlq_n_u32::<16>(vandq_u32(w, vdupq_n_u32(0x8000)));
        let em = vandq_u32(w, vdupq_n_u32(0x7fff));
        let scaled = vmulq_f32(vreinterpretq_f32_u32(vshlq_n_u32::<13>(em)), vdupq_n_f32(f32::from_bits(0x7780_0000)));
        let inf_nan = vandq_u32(vcgeq_u32(em, vdupq_n_u32(0x7c00)), vdupq_n_u32(0x7f80_0000));
        let out = vreinterpretq_f32_u32(vorrq_u32(vorrq_u32(vreinterpretq_u32_f32(scaled), inf_nan), sign));
        let mut r = [0f32; 4];
        vst1q_f32(r.as_mut_ptr(), out);
        r
    }
}

#[cfg(target_arch = "aarch64")]
fn main() {
    let mut bad = 0u64;
    let mut n = 0u64;
    let xs = f64s(400_000);
    let mut s = 7u64;
    for &x in &xs {
        let y = xs[(rng(&mut s) as usize) % xs.len()];
        let z = xs[(rng(&mut s) as usize) % xs.len()];
        let (xf, yf, zf) = (x as f32, y as f32, z as f32);
        let checks = [
            same64(old::fma(x, y, z), libm::fma(x, y, z)),
            same32(old::fmaf(xf, yf, zf), libm::fmaf(xf, yf, zf)),
            same64(old::rint(x), libm::rint(x)),
            same32(old::rintf(xf), libm::rintf(xf)),
            same64(old::sqrt(x), libm::sqrt(x)),
            same32(old::sqrtf(xf), libm::sqrtf(xf)),
            old::f64_to_f16(x) == makepad_half::f16::from_f64(x).to_bits() || (x.is_nan() && makepad_half::f16::from_f64(x).is_nan()),
            old::f32_to_f16(xf) == makepad_half::f16::from_f32(xf).to_bits() || (xf.is_nan() && makepad_half::f16::from_f32(xf).is_nan()),
        ];
        for (k, c) in checks.iter().enumerate() {
            n += 1;
            if !c {
                bad += 1;
                if bad < 20 {
                    eprintln!("differs: check {k} x={x:e} y={y:e} z={z:e}");
                }
            }
        }
    }
    // every f16: conversion up, and arithmetic on a sample of pairs
    for i in 0..=u16::MAX {
        n += 1;
        let a = makepad_half::f16::from_bits(i);
        if !same32(old::f16_to_f32(i), makepad_half::f16::from_bits(i).to_f32()) {
            bad += 1;
        }
        let quad = [i, i ^ 0x8000, i.wrapping_add(1), i.wrapping_mul(3)];
        let (o, nw) = (old::fcvtl(quad), new_fcvtl(quad));
        for k in 0..4 {
            n += 1;
            // FCVTL quiets a signalling NaN; the replacement keeps the payload: both NaN
            if !same32(o[k], nw[k]) {
                bad += 1;
                if bad < 20 {
                    eprintln!("fcvtl differs at {:#06x}: {:#010x} vs {:#010x}", quad[k], o[k].to_bits(), nw[k].to_bits());
                }
            }
        }
        for _ in 0..8 {
            let j = rng(&mut s) as u16;
            let b = makepad_half::f16::from_bits(j);
            for op in 0..4u8 {
                n += 1;
                let new = match op {
                    0 => a + b,
                    1 => a - b,
                    2 => a * b,
                    _ => a / b,
                };
                let old = old::f16_op(op, i, j);
                if old != new.to_bits() && !(makepad_half::f16::from_bits(old).is_nan() && new.is_nan()) {
                    bad += 1;
                    if bad < 20 {
                        eprintln!("f16 op {op} {:#06x} {:#06x}: {:#06x} vs {:#06x}", i, j, old, new.to_bits());
                    }
                }
            }
        }
    }
    println!("asm-identity aarch64: {n} checks, {bad} differ");
    std::process::exit(if bad == 0 { 0 } else { 1 });
}

#[cfg(target_arch = "x86_64")]
mod old {
    use core::arch::asm;
    pub fn sqrt(mut x: f64) -> f64 {
        unsafe { asm!("sqrtsd {x}, {x}", x = inout(xmm_reg) x, options(nostack, nomem, pure)) };
        x
    }
    pub fn sqrtf(mut x: f32) -> f32 {
        unsafe { asm!("sqrtss {x}, {x}", x = inout(xmm_reg) x, options(nostack, nomem, pure)) };
        x
    }
    #[target_feature(enable = "fma")]
    pub unsafe fn fma(mut x: f64, y: f64, z: f64) -> f64 {
        asm!("vfmadd213sd {x}, {y}, {z}", x = inout(xmm_reg) x, y = in(xmm_reg) y, z = in(xmm_reg) z, options(nostack, nomem, pure));
        x
    }
    #[target_feature(enable = "fma")]
    pub unsafe fn fmaf(mut x: f32, y: f32, z: f32) -> f32 {
        asm!("vfmadd213ss {x}, {y}, {z}", x = inout(xmm_reg) x, y = in(xmm_reg) y, z = in(xmm_reg) z, options(nostack, nomem, pure));
        x
    }
}

#[cfg(target_arch = "x86_64")]
fn main() {
    let fma_hw = std::arch::is_x86_feature_detected!("fma");
    let mut bad = 0u64;
    let mut n = 0u64;
    let xs = f64s(1_000_000);
    let mut s = 7u64;
    for &x in &xs {
        let y = xs[(rng(&mut s) as usize) % xs.len()];
        let z = xs[(rng(&mut s) as usize) % xs.len()];
        let (xf, yf, zf) = (x as f32, y as f32, z as f32);
        let mut checks = vec![same64(old::sqrt(x), libm::sqrt(x)), same32(old::sqrtf(xf), libm::sqrtf(xf))];
        if fma_hw {
            checks.push(same64(unsafe { old::fma(x, y, z) }, libm::fma(x, y, z)));
            checks.push(same32(unsafe { old::fmaf(xf, yf, zf) }, libm::fmaf(xf, yf, zf)));
        }
        for c in checks {
            n += 1;
            if !c {
                bad += 1;
                if bad < 20 {
                    eprintln!("differs: x={x:e} y={y:e} z={z:e}");
                }
            }
        }
    }
    println!("asm-identity x86_64: {n} checks (fma {}), {bad} differ", if fma_hw { "hardware" } else { "fallback" });
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
