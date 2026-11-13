// The fused multiply-adds of aarch64 NEON against the CPU: which operand's NaN a result carries,
// and with which sign. On Arm a NaN result is the first signaling NaN among the operands in the
// instruction's order, else the first quiet one, so the order a std passes the operands in is
// visible. vfmaq_f32 and vfmsq_f32 passed b before c where rustc's code passes c first, so a NaN in
// both came out as b's under the JIT and as c's under rustc; vfmsq_f64 negated b where rustc's
// negates c, so a NaN from either came out with the other sign. Every operand takes 1.5, infinity,
// zero, a quiet NaN and a signaling NaN, with a payload that names it (a: 1, b: 2, c: 3); each line
// is one choice and every op's result bits. The operands pass through black_box so the CPU computes
// them. Differential file: `| ` lines equal under rustc --test and makepad test.
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
use std::hint::black_box;
use std::mem::transmute;

fn f32s(who: u32) -> [f32; 5] {
    [1.5, f32::INFINITY, 0.0, f32::from_bits(0x7fc0_0000 | who), f32::from_bits(0x7f80_0000 | who)]
}

fn f64s(who: u64) -> [f64; 5] {
    [1.5, f64::INFINITY, 0.0, f64::from_bits(0x7ff8_0000_0000_0000 | who), f64::from_bits(0x7ff0_0000_0000_0000 | who)]
}

fn q32(x: f32) -> float32x4_t {
    unsafe { transmute(black_box([x; 4])) }
}

fn q64(x: f64) -> float64x2_t {
    unsafe { transmute(black_box([x; 2])) }
}

fn lane32(v: float32x4_t) -> u32 {
    let r: [f32; 4] = unsafe { transmute(v) };
    r[0].to_bits()
}

fn lane64(v: float64x2_t) -> u64 {
    let r: [f64; 2] = unsafe { transmute(v) };
    r[0].to_bits()
}

#[test]
fn a01_the_nan_a_fused_multiply_add_passes_on() {
    let (a32, b32, c32) = (f32s(1), f32s(2), f32s(3));
    let (a64, b64, c64) = (f64s(1), f64s(2), f64s(3));
    for i in 0..5 {
        for j in 0..5 {
            for k in 0..5 {
                let (a, b, c) = (q32(a32[i]), q32(b32[j]), q32(c32[k]));
                let (fma, fms) = unsafe { (vfmaq_f32(a, b, c), vfmsq_f32(a, b, c)) };
                let lane = unsafe { vfmaq_laneq_f32::<0>(a, b, c) };
                let (a, b, c) = (q64(a64[i]), q64(b64[j]), q64(c64[k]));
                let (fma64, fms64) = unsafe { (vfmaq_f64(a, b, c), vfmsq_f64(a, b, c)) };
                println!(
                    "| a{i} b{j} c{k}: vfmaq_f32 {:08x} vfmsq_f32 {:08x} vfmaq_laneq_f32 {:08x} vfmaq_f64 {:016x} vfmsq_f64 {:016x}",
                    lane32(fma), lane32(fms), lane32(lane), lane64(fma64), lane64(fms64)
                );
            }
        }
    }
}
