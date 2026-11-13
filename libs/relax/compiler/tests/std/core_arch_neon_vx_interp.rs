// The aarch64 NEON intrinsics relax-codegen's vector interpreter (libs/relax/codegen/src/interp/vx.rs)
// calls that the std here lacked, so the codegen crate did not compile under the JIT on an aarch64
// host ("cannot find value `vrndaq_f32` in this scope" and fifteen more): vmovl_high_{u8,s8,u16,
// s16,u32,s32}, vpaddlq_u16, vpaddlq_u32, vaddvq_u8, vaddvq_u64, vrhaddq_u32, vabdq_u8,
// vrndaq_f32, vrndaq_f64, vcvt_f32_f64 and vcvt_f64_f32. Each is folded over edge and
// pseudo-random operands into one hash per intrinsic; the float ones also print every edge
// result's bits (ties, signed zeros, infinities, quiet and signaling NaNs, subnormals, overflow).
// Differential file: `| ` lines equal under rustc --test (the CPU's instructions) and makepad test.
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
use std::hint::black_box;
use std::mem::transmute;

const CASES: usize = 2048;

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// 16 bytes for case `case`: the first cases spread the byte edges over every lane, the rest are
/// random.
fn bytes(s: &mut u64, case: usize) -> [u8; 16] {
    const EDGE: [u8; 8] = [0, 1, 0x7f, 0x80, 0x81, 0xfe, 0xff, 0x55];
    let mut b = [0u8; 16];
    for (i, x) in b.iter_mut().enumerate() {
        *x = if case < 64 { EDGE[(case + i * 3 + (i >> 2) * case) % 8] } else { next(s) as u8 };
    }
    // wide lanes at their edges too: all ones, the sign bit alone, zero
    if case >= 64 && case % 7 == 0 {
        let lane = (next(s) % 4) as usize * 4;
        let v: [u8; 4] = [[0xff; 4], [0, 0, 0, 0x80], [0; 4], [0xff, 0xff, 0xff, 0x7f]][(next(s) % 4) as usize];
        b[lane..lane + 4].copy_from_slice(&v);
    }
    b
}

#[derive(Clone, Copy)]
struct Fold(u64);
impl Fold {
    fn add(&mut self, v: &[u8]) {
        for x in v {
            self.0 = (self.0 ^ *x as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
}

// inside an unsafe block
macro_rules! fold16 {
    ($h:expr, $v:expr) => {{
        let b: [u8; 16] = transmute($v);
        $h.add(&b);
    }};
}

fn hex<T: Copy + Into<u64>>(v: &[T], digits: usize) -> String {
    v.iter().map(|x| format!(" {:0width$x}", (*x).into(), width = digits)).collect()
}

#[test]
fn a01_widening_pairwise_and_across() {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut h = [Fold(0xcbf2_9ce4_8422_2325); 12];
    for case in 0..CASES {
        let (x, y) = (black_box(bytes(&mut s, case)), black_box(bytes(&mut s, case + 1)));
        unsafe {
            let (u8a, u8b): (uint8x16_t, uint8x16_t) = (transmute(x), transmute(y));
            fold16!(h[0], vmovl_high_u8(u8a));
            fold16!(h[1], vmovl_high_s8(transmute::<_, int8x16_t>(x)));
            fold16!(h[2], vmovl_high_u16(transmute::<_, uint16x8_t>(x)));
            fold16!(h[3], vmovl_high_s16(transmute::<_, int16x8_t>(x)));
            fold16!(h[4], vmovl_high_u32(transmute::<_, uint32x4_t>(x)));
            fold16!(h[5], vmovl_high_s32(transmute::<_, int32x4_t>(x)));
            fold16!(h[6], vpaddlq_u16(transmute::<_, uint16x8_t>(x)));
            fold16!(h[7], vpaddlq_u32(transmute::<_, uint32x4_t>(x)));
            h[8].add(&[vaddvq_u8(u8a)]);
            h[9].add(&vaddvq_u64(transmute::<_, uint64x2_t>(x)).to_le_bytes());
            fold16!(h[10], vrhaddq_u32(transmute::<_, uint32x4_t>(x), transmute::<_, uint32x4_t>(y)));
            fold16!(h[11], vabdq_u8(u8a, u8b));
        }
    }
    let names = [
        "vmovl_high_u8", "vmovl_high_s8", "vmovl_high_u16", "vmovl_high_s16", "vmovl_high_u32", "vmovl_high_s32",
        "vpaddlq_u16", "vpaddlq_u32", "vaddvq_u8", "vaddvq_u64", "vrhaddq_u32", "vabdq_u8",
    ];
    for (n, f) in names.iter().zip(&h) {
        println!("| {n} {:016x}", f.0);
    }
    // a few by hand
    unsafe {
        let ones: uint32x4_t = transmute([u32::MAX; 4]);
        let r: [u32; 4] = transmute(vrhaddq_u32(ones, transmute([u32::MAX, 0, 1, 2u32])));
        println!("| vrhaddq_u32(MAX, [MAX 0 1 2]) {r:?}");
        let r: [u64; 2] = transmute(vpaddlq_u32(ones));
        println!("| vpaddlq_u32(MAX) {r:?}");
        println!("| vaddvq_u8(0xff) {}", vaddvq_u8(transmute([0xffu8; 16])));
        println!("| vaddvq_u64(MAX, 2) {}", vaddvq_u64(transmute([u64::MAX, 2])));
        let r: [i16; 8] = transmute(vmovl_high_s8(transmute([0i8, 0, 0, 0, 0, 0, 0, 0, -128, -1, 0, 1, 127, -2, 2, 5])));
        println!("| vmovl_high_s8 {r:?}");
    }
}

const F32: [u32; 24] = [
    0x0000_0000, 0x8000_0000, 0x3f00_0000, 0xbf00_0000, 0x3fc0_0000, 0x4020_0000, 0xc020_0000, 0x3eff_ffff, 0x4b00_0001,
    0x4b7f_ffff, 0xcb00_0001, 0x7f7f_ffff, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0xffc0_1234, 0x7f80_0001, 0xff80_4321,
    0x0000_0001, 0x807f_ffff, 0x0080_0000, 0x3f7f_ffff, 0x3f80_0001, 0xc0b0_0000,
];
const F64: [u64; 24] = [
    0x0000_0000_0000_0000, 0x8000_0000_0000_0000, 0x3fe0_0000_0000_0000, 0xbfe0_0000_0000_0000, 0x3ff8_0000_0000_0000,
    0x4004_0000_0000_0000, 0xc004_0000_0000_0000, 0x3fdf_ffff_ffff_ffff, 0x4330_0000_0000_0001, 0xc330_0000_0000_0001,
    // f32 rounding: a tie to even down, a tie to even up, just past a tie, the f32 maximum's tie and past it
    0x3ff0_0000_1000_0000, 0x3ff0_0000_3000_0000, 0x3ff0_0000_1000_0001, 0x47ef_ffff_f000_0000, 0x47ef_ffff_f000_0001,
    0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000, 0x7ff8_0000_0000_0000, 0xfff8_1234_5678_9abc, 0x7ff0_0000_2000_0001,
    // the f32 subnormals: the smallest, half of it (a tie to zero), below it, and an f64 subnormal
    0x36a0_0000_0000_0000, 0x3690_0000_0000_0000, 0x3680_0000_0000_0000, 0x0000_0000_0000_0001,
];

#[test]
fn a02_round_ties_away_and_convert() {
    unsafe {
        for c in F32.chunks(4) {
            let a: float32x4_t = transmute([c[0], c[1], c[2], c[3]]);
            let r: [u32; 4] = transmute(vrndaq_f32(black_box(a)));
            println!("| vrndaq_f32{} ->{}", hex(c, 8), hex(&r, 8));
        }
        for c in F64.chunks(2) {
            let a: float64x2_t = transmute([c[0], c[1]]);
            let r: [u64; 2] = transmute(vrndaq_f64(black_box(a)));
            println!("| vrndaq_f64{} ->{}", hex(c, 16), hex(&r, 16));
            let n: [u32; 2] = transmute(vcvt_f32_f64(black_box(a)));
            println!("| vcvt_f32_f64{} ->{}", hex(c, 16), hex(&n, 8));
        }
        for c in F32.chunks(2) {
            let a: float32x2_t = transmute([c[0], c[1]]);
            let w: [u64; 2] = transmute(vcvt_f64_f32(black_box(a)));
            println!("| vcvt_f64_f32{} ->{}", hex(c, 8), hex(&w, 16));
        }
        // random bit patterns, NaNs and all
        let mut s = 0x2545_f491_4f6c_dd1du64;
        let mut h = [Fold(0xcbf2_9ce4_8422_2325); 4];
        for _ in 0..CASES {
            let (p, q) = (next(&mut s), next(&mut s));
            let f4: float32x4_t = transmute([p, q]);
            let d2: float64x2_t = transmute([p, q]);
            fold16!(h[0], vrndaq_f32(black_box(f4)));
            fold16!(h[1], vrndaq_f64(black_box(d2)));
            h[2].add(&transmute::<float32x2_t, [u8; 8]>(vcvt_f32_f64(black_box(d2))));
            fold16!(h[3], vcvt_f64_f32(black_box(transmute::<u64, float32x2_t>(p))));
        }
        for (n, f) in ["vrndaq_f32", "vrndaq_f64", "vcvt_f32_f64", "vcvt_f64_f32"].iter().zip(&h) {
            println!("| {n} random {:016x}", f.0);
        }
    }
}
