// The aarch64 NEON widening add across a vector (vaddlv_u8, vaddlvq_u8), reciprocal estimate
// (vrecpe_f32, vrecpeq_f32: Arm's FRECPE, an 8-bit estimate) and reciprocal step (vrecps_f32,
// vrecpsq_f32: FRECPS, 2 - a * b rounded once), reciprocal square-root estimate (vrsqrte_f32,
// vrsqrteq_f32: FRSQRTE) and step (vrsqrts_f32, vrsqrtsq_f32: FRSQRTS, (3 - a * b) / 2 rounded
// once), against rustc's std::arch: fixed values asserted, and every class of f32 (zeros,
// infinities, NaNs, subnormals at the 2^-128 overflow edge, the top exponents, random bits) as `| `
// lines, equal under rustc --test and makepad test on an aarch64 machine (relax issue 71 and PR 80:
// the first three were missing; relax issue 97: the square-root pair).
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

fn recpe(x: u32) -> u32 {
    unsafe {
        let v: float32x4_t = std::mem::transmute([f32::from_bits(x); 4]);
        let r: [f32; 4] = std::mem::transmute(vrecpeq_f32(v));
        let h: [f32; 2] = std::mem::transmute(vrecpe_f32(std::mem::transmute([f32::from_bits(x); 2])));
        assert!(r.iter().chain(h.iter()).all(|l| l.to_bits() == r[0].to_bits()), "lanes of {x:08x} differ: {r:?} {h:?}");
        r[0].to_bits()
    }
}

#[test]
fn a01_add_across_widening() {
    unsafe {
        assert_eq!(vaddlv_u8(vdup_n_u8(0)), 0);
        assert_eq!(vaddlvq_u8(vdupq_n_u8(0)), 0);
        assert_eq!(vaddlv_u8(vdup_n_u8(1)), 8);
        assert_eq!(vaddlvq_u8(vdupq_n_u8(1)), 16);
        assert_eq!(vaddlvq_u8(vdupq_n_u8(255)), 4080);
        assert_eq!(vaddlv_u8(vdup_n_u8(255)), 2040);
        let v: uint8x16_t = std::mem::transmute([0u8, 1, 2, 3, 250, 251, 252, 253, 254, 255, 7, 9, 128, 127, 64, 32]);
        assert_eq!(vaddlvq_u8(v), 1888);
        let w: uint8x8_t = std::mem::transmute([200u8, 100, 50, 25, 255, 0, 1, 2]);
        assert_eq!(vaddlv_u8(w), 633);
        println!("| addlv {} {} {}", vaddlvq_u8(v), vaddlv_u8(w), vaddlvq_u8(vdupq_n_u8(17)));
    }
}

#[test]
fn a02_reciprocal_estimate_classes() {
    // the issue's: 1/0.5 is 1 + 255/256 (a NEON call outside a `neon` function is unsafe)
    unsafe {
        let r = vrecpeq_f32(vdupq_n_f32(0.5));
        assert_eq!(vgetq_lane_f32(r, 0), 1.99609375);
    }
    assert_eq!(recpe(2.0f32.to_bits()), 0.4990234375f32.to_bits());
    assert_eq!(recpe(0), f32::INFINITY.to_bits());
    assert_eq!(recpe(0x8000_0000), f32::NEG_INFINITY.to_bits());
    assert_eq!(recpe(f32::INFINITY.to_bits()), 0);
    assert_eq!(recpe(f32::NEG_INFINITY.to_bits()), 0x8000_0000);
    assert_eq!(recpe(0x7f80_0001), 0x7fc0_0001, "a signalling NaN comes back quieted");
    assert_eq!(recpe(0xffc0_1234), 0xffc0_1234, "a quiet NaN comes back as it was");
    let edges = [
        0x0000_0001, 0x001f_ffff, 0x0020_0000, 0x0020_0001, 0x003f_ffff, 0x0040_0000, 0x0055_5555, 0x007f_ffff,
        0x0080_0000, 0x0080_0001, 0x3f00_0000, 0x3f80_0000, 0x3fc0_0000, 0x3fff_ffff, 0x4049_0fdb, 0x3eaa_aaab,
        0x7e7f_ffff, 0x7e80_0000, 0x7eff_ffff, 0x7f00_0000, 0x7f40_0000, 0x7f7f_ffff, 0x8020_0000, 0x801f_ffff,
        0xbf80_0000, 0xff7f_ffff, 0xfe80_0000,
    ];
    for x in edges {
        println!("| recpe {:08x} {:08x}", x, recpe(x));
    }
}

#[test]
fn a03_reciprocal_estimate_fractions_and_random() {
    // every 8-bit fraction prefix the estimate reads, at a low, a middle and a top exponent
    let mut d = 0xcbf2_9ce4_8422_2325u64;
    let mut n = 0;
    let mut eat = |v: u32| {
        for b in v.to_le_bytes() {
            d ^= b as u64;
            d = d.wrapping_mul(0x100_0000_01b3);
        }
        n += 1;
    };
    for exp in [1u32, 127, 200, 252, 253, 254] {
        for top in 0..256u32 {
            let x = (exp << 23) | (top << 15) | 0x4321;
            eat(recpe(x));
            eat(recpe(x | 0x8000_0000));
        }
    }
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..4000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        eat(recpe((s >> 16) as u32));
    }
    println!("| recpe sweep {} {:016x}", n, d);
}

fn recps(a: u32, b: u32) -> u32 {
    unsafe {
        let (va, vb) = ([f32::from_bits(a); 4], [f32::from_bits(b); 4]);
        let r: [f32; 4] = std::mem::transmute(vrecpsq_f32(std::mem::transmute(va), std::mem::transmute(vb)));
        let (ha, hb) = ([f32::from_bits(a); 2], [f32::from_bits(b); 2]);
        let h: [f32; 2] = std::mem::transmute(vrecps_f32(std::mem::transmute(ha), std::mem::transmute(hb)));
        assert!(r.iter().chain(h.iter()).all(|l| l.to_bits() == r[0].to_bits()), "lanes of {a:08x} {b:08x} differ: {r:?} {h:?}");
        r[0].to_bits()
    }
}

#[test]
fn a04_reciprocal_step() {
    // PR 80's: 2 - 0.5 * 2 is 1
    unsafe {
        let step = vrecpsq_f32(vdupq_n_f32(0.5), vdupq_n_f32(2.0));
        assert_eq!(vgetq_lane_f32(step, 0), 1.0);
    }
    // fused: (1 + 2^-23) * (1 - 2^-24) rounded first would give 2 - 1 = 1
    assert_eq!(recps(0x3f80_0001, 0x3f7f_ffff), 0x3f7f_ffff);
    assert_eq!(recps(f32::INFINITY.to_bits(), 0), 2.0f32.to_bits(), "an infinity times a zero is 2");
    assert_eq!(recps(0x8000_0000, f32::NEG_INFINITY.to_bits()), 2.0f32.to_bits());
    assert_eq!(recps(f32::INFINITY.to_bits(), 1.0f32.to_bits()), f32::NEG_INFINITY.to_bits());
    let v = [
        0u32, 0x8000_0000, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0x7f80_0001, 0xffc0_1234, 0x7fa0_0005, 0x0000_0001,
        0x807f_ffff, 0x3f80_0000, 0x3f80_0001, 0x3f7f_ffff, 0x4000_0000, 0x7f7f_ffff, 0x0080_0000, 0x3f00_0000,
        0xbfc0_0000, 0x5f00_0000, 0x2000_0000,
    ];
    let mut d = 0xcbf2_9ce4_8422_2325u64;
    let mut n = 0;
    let mut eat = |v: u32| {
        for b in v.to_le_bytes() {
            d ^= b as u64;
            d = d.wrapping_mul(0x100_0000_01b3);
        }
        n += 1;
    };
    for &a in &v {
        println!("| recps {:08x} {:08x} {:08x} {:08x}", a, recps(a, 0), recps(a, 0x7f80_0000), recps(a, 0x3f80_0001));
        for &b in &v {
            eat(recps(a, b));
        }
    }
    // the refinement as code runs it: x against its own estimate, near one and across exponents
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..4000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let x = (s >> 16) as u32;
        eat(recps(x, recpe(x)));
        eat(recps(x, (s >> 40) as u32));
    }
    println!("| recps sweep {} {:016x}", n, d);
}

fn rsqrte(x: u32) -> u32 {
    unsafe {
        let v: float32x4_t = std::mem::transmute([f32::from_bits(x); 4]);
        let r: [f32; 4] = std::mem::transmute(vrsqrteq_f32(v));
        let h: [f32; 2] = std::mem::transmute(vrsqrte_f32(std::mem::transmute([f32::from_bits(x); 2])));
        assert!(r.iter().chain(h.iter()).all(|l| l.to_bits() == r[0].to_bits()), "lanes of {x:08x} differ: {r:?} {h:?}");
        r[0].to_bits()
    }
}

fn rsqrts(a: u32, b: u32) -> u32 {
    unsafe {
        let (va, vb) = ([f32::from_bits(a); 4], [f32::from_bits(b); 4]);
        let r: [f32; 4] = std::mem::transmute(vrsqrtsq_f32(std::mem::transmute(va), std::mem::transmute(vb)));
        let (ha, hb) = ([f32::from_bits(a); 2], [f32::from_bits(b); 2]);
        let h: [f32; 2] = std::mem::transmute(vrsqrts_f32(std::mem::transmute(ha), std::mem::transmute(hb)));
        assert!(r.iter().chain(h.iter()).all(|l| l.to_bits() == r[0].to_bits()), "lanes of {a:08x} {b:08x} differ: {r:?} {h:?}");
        r[0].to_bits()
    }
}

#[test]
fn a05_reciprocal_sqrt_estimate() {
    // relax issue 97's: the estimate of 1/sqrt(4) and one refinement step of it against itself
    unsafe {
        let e = vrsqrteq_f32(vdupq_n_f32(4.0));
        let s = vrsqrtsq_f32(e, e);
        assert_eq!((vgetq_lane_f32(e, 0), vgetq_lane_f32(s, 0)), (0.49902344, 1.3754878));
    }
    assert_eq!(rsqrte(0), f32::INFINITY.to_bits());
    assert_eq!(rsqrte(0x8000_0000), f32::NEG_INFINITY.to_bits());
    assert_eq!(rsqrte(f32::INFINITY.to_bits()), 0);
    assert_eq!(rsqrte(f32::NEG_INFINITY.to_bits()), 0x7fc0_0000, "a negative number is the default NaN");
    assert_eq!(rsqrte((-4.0f32).to_bits()), 0x7fc0_0000);
    assert_eq!(rsqrte(0x7f80_0001), 0x7fc0_0001, "a signalling NaN comes back quieted");
    assert_eq!(rsqrte(0xffc0_1234), 0xffc0_1234, "a quiet NaN comes back as it was");
    let edges = [
        0x0000_0001, 0x0000_0002, 0x0000_0003, 0x0000_ffff, 0x0020_0000, 0x003f_ffff, 0x0040_0000, 0x007f_ffff,
        0x0080_0000, 0x0080_0001, 0x0100_0000, 0x3e80_0000, 0x3f00_0000, 0x3f80_0000, 0x3fc0_0000, 0x3fff_ffff,
        0x4000_0000, 0x4049_0fdb, 0x3eaa_aaab, 0x7e7f_ffff, 0x7f00_0000, 0x7f7f_ffff, 0x8000_0001, 0xbf80_0000,
    ];
    for x in edges {
        println!("| rsqrte {:08x} {:08x}", x, rsqrte(x));
    }
    // every fraction prefix the estimate reads, at even and odd exponents (each half of the
    // scaled range), subnormals by their leading bit, and random bits
    let mut d = 0xcbf2_9ce4_8422_2325u64;
    let mut n = 0;
    let mut eat = |v: u32| {
        for b in v.to_le_bytes() {
            d ^= b as u64;
            d = d.wrapping_mul(0x100_0000_01b3);
        }
        n += 1;
    };
    for exp in [1u32, 2, 126, 127, 253, 254] {
        for top in 0..512u32 {
            eat(rsqrte((exp << 23) | (top << 14) | 0x1234));
        }
    }
    for lead in 0..23u32 {
        eat(rsqrte(1 << lead));
        eat(rsqrte((1 << lead) | ((1 << lead) - 1)));
    }
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..4000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        eat(rsqrte((s >> 16) as u32));
    }
    println!("| rsqrte sweep {} {:016x}", n, d);
}

#[test]
fn a06_reciprocal_sqrt_step() {
    // fused: (1 + 2^-23) * (2 - 2^-23) rounded first would give (3 - 2) / 2 = 0.5
    assert_eq!(rsqrts(0x3f80_0001, 0x3fff_ffff), 0x3eff_fffe);
    assert_eq!(rsqrts(f32::INFINITY.to_bits(), 0), 1.5f32.to_bits(), "an infinity times a zero is 1.5");
    assert_eq!(rsqrts(0x8000_0000, f32::NEG_INFINITY.to_bits()), 1.5f32.to_bits());
    assert_eq!(rsqrts(f32::INFINITY.to_bits(), 1.0f32.to_bits()), f32::NEG_INFINITY.to_bits());
    assert_eq!(rsqrts(1.5f32.to_bits(), 2.0f32.to_bits()), 0, "3 - 3 is +0");
    // a product past the top of the range whose half is not: (3 + 2 * MAX) / 2 rounds to MAX
    assert_eq!(rsqrts((-f32::MAX).to_bits(), 2.0f32.to_bits()), f32::MAX.to_bits());
    let v = [
        0u32, 0x8000_0000, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0x7f80_0001, 0xffc0_1234, 0x7fa0_0005, 0x0000_0001,
        0x807f_ffff, 0x3f80_0000, 0x3f80_0001, 0x3f7f_ffff, 0x4000_0000, 0x7f7f_ffff, 0xff7f_ffff, 0x0080_0000,
        0x0100_0000, 0x00ff_ffff, 0x3f00_0000, 0xbfc0_0000, 0x5f00_0000, 0x2000_0000, 0x4040_0000,
    ];
    let mut d = 0xcbf2_9ce4_8422_2325u64;
    let mut n = 0;
    let mut eat = |v: u32| {
        for b in v.to_le_bytes() {
            d ^= b as u64;
            d = d.wrapping_mul(0x100_0000_01b3);
        }
        n += 1;
    };
    for &a in &v {
        println!("| rsqrts {:08x} {:08x} {:08x} {:08x}", a, rsqrts(a, 0), rsqrts(a, 0x7f7f_ffff), rsqrts(a, 0x3f80_0001));
        for &b in &v {
            eat(rsqrts(a, b));
        }
    }
    // the refinement as code runs it (x * e against e), products near 3 and near the top of the
    // range, and random pairs
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..4000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let x = (s >> 16) as u32;
        let e = rsqrte(x & 0x7fff_ffff);
        eat(rsqrts((f32::from_bits(x & 0x7fff_ffff) * f32::from_bits(e)).to_bits(), e));
        eat(rsqrts(x, (s >> 40) as u32));
        eat(rsqrts(0x7f00_0000 | (x & 0x807f_ffff), 0x4000_0000 | ((s >> 40) as u32 & 0x807f_ffff)));
    }
    println!("| rsqrts sweep {} {:016x}", n, d);
}
