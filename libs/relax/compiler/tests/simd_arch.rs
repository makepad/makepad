// core::arch intrinsics lowered to vector instructions (libs/relax/compiler/src/simd.rs) against
// rustc's real ones: one line per intrinsic, "<name> <hash>" over xorshift inputs mixed with
// edge lanes (0, all ones, sign bits, 0x7f..). rustc -O --test and `makepad test` print the
// same lines (diff them); MAKEPAD_RUN_NO_SIMD=1 gives std's scalar versions.
// Each intrinsic runs twice: through a closure (inlined: vector values flow between
// intrinsics) and through a fn pointer (the intrinsic's own compiled body).

#![allow(unused_unsafe, non_snake_case, unused)]

#[cfg(target_arch = "aarch64")]
use core::arch::aarch64::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

const N: usize = 512;

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// random bytes with edge lanes mixed in
fn rnd<T: Copy>(s: &mut u64) -> T {
    let mut buf = [0u64; 8];
    let mode = next(s) % 8;
    for w in buf.iter_mut() {
        let r = next(s);
        *w = match mode {
            0 => 0,
            1 => u64::MAX,
            2 => 0x8000_8000_8000_8000 ^ (r & 0x0101_0101_0101_0101),
            3 => 0x7f7f_7fff_7fff_ffff ^ (r & 0x0100_0001_0000_0080),
            4 => r & 0x00ff_00ff_00ff_00ff,
            _ => r,
        };
    }
    unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const T) }
}

fn mix<T>(h: &mut u64, v: &T) {
    let b = unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) };
    for &x in b {
        *h = (*h ^ x as u64).wrapping_mul(0x100_0000_01b3);
    }
}

fn un<A: Copy, R, F: Fn(A) -> R>(name: &str, f: F, p: fn(A) -> R) {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut hp = h;
    for _ in 0..N {
        let a: A = rnd(&mut s);
        mix(&mut h, &f(a));
        mix(&mut hp, &p(core::hint::black_box(a)));
    }
    println!("{} {:016x} {}", name, h, if h == hp { "" } else { "PTR-DIFFERS" });
}

fn bin<A: Copy, B: Copy, R, F: Fn(A, B) -> R>(name: &str, f: F, p: fn(A, B) -> R) {
    let mut s = 0x2545_f491_4f6c_dd1du64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut hp = h;
    for _ in 0..N {
        let a: A = rnd(&mut s);
        let b: B = rnd(&mut s);
        mix(&mut h, &f(a, b));
        mix(&mut hp, &p(core::hint::black_box(a), b));
    }
    println!("{} {:016x} {}", name, h, if h == hp { "" } else { "PTR-DIFFERS" });
}

fn tri<A: Copy, B: Copy, C: Copy, R, F: Fn(A, B, C) -> R>(name: &str, f: F, p: fn(A, B, C) -> R) {
    let mut s = 0x1234_5678_9abc_def1u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut hp = h;
    for _ in 0..N {
        let a: A = rnd(&mut s);
        let b: B = rnd(&mut s);
        let c: C = rnd(&mut s);
        mix(&mut h, &f(a, b, c));
        mix(&mut hp, &p(core::hint::black_box(a), b, c));
    }
    println!("{} {:016x} {}", name, h, if h == hp { "" } else { "PTR-DIFFERS" });
}

/// A float lane's bits, for `fma`.
trait Lane: Copy + PartialEq {
    const MARKER: Self;
    fn nan(self) -> bool;
    fn quieted(self) -> Self;
}
impl Lane for u32 {
    const MARKER: u32 = 0x7fc0_0000;
    fn nan(self) -> bool {
        self & 0x7fff_ffff > 0x7f80_0000
    }
    fn quieted(self) -> u32 {
        self | 0x0040_0000
    }
}
impl Lane for u64 {
    const MARKER: u64 = 0x7ff8_0000_0000_0000;
    fn nan(self) -> bool {
        self & 0x7fff_ffff_ffff_ffff > 0x7ff0_0000_0000_0000
    }
    fn quieted(self) -> u64 {
        self | 0x0008_0000_0000_0000
    }
}

/// `tri` for a packed fused multiply-add of L lanes of E's bits. A lane with two or more NaN
/// operands may return any of them: llvm.fma leaves the choice open and rustc's pick depends on
/// register allocation (b's NaN at 128 bits, a's at 256). Such a lane hashes as one marker when its
/// result is one of those NaNs, quieted; any other result still shows.
fn fma<V: Copy, E: Lane, const L: usize>(name: &str, f: impl Fn(V, V, V) -> V, p: fn(V, V, V) -> V) {
    let lanes = |v: V| -> [E; L] { unsafe { core::ptr::read_unaligned(&v as *const V as *const [E; L]) } };
    let masked = |r: V, a: V, b: V, c: V| -> [E; L] {
        let (mut r, a, b, c) = (lanes(r), lanes(a), lanes(b), lanes(c));
        for l in 0..L {
            let ops = [a[l], b[l], c[l]];
            let nans = ops.iter().filter(|x| x.nan());
            if nans.clone().count() >= 2 && nans.clone().any(|x| x.quieted() == r[l]) {
                r[l] = E::MARKER;
            }
        }
        r
    };
    let mut s = 0x1234_5678_9abc_def1u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut hp = h;
    for _ in 0..N {
        let a: V = rnd(&mut s);
        let b: V = rnd(&mut s);
        let c: V = rnd(&mut s);
        mix(&mut h, &masked(f(a, b, c), a, b, c));
        mix(&mut hp, &masked(p(core::hint::black_box(a), b, c), a, b, c));
    }
    println!("{} {:016x} {}", name, h, if h == hp { "" } else { "PTR-DIFFERS" });
}

macro_rules! u {
    ($name:ident, $a:ty) => {
        un(stringify!($name), |a: $a| unsafe { $name(a) }, |a: $a| unsafe { $name(a) })
    };
    ($name:ident :: <$k:literal>, $a:ty) => {
        un(concat!(stringify!($name), "::<", $k, ">"), |a: $a| unsafe { $name::<$k>(a) }, |a: $a| unsafe { $name::<$k>(a) })
    };
}
macro_rules! b {
    ($name:ident, $a:ty, $b:ty) => {
        bin(stringify!($name), |a: $a, b: $b| unsafe { $name(a, b) }, |a: $a, b: $b| unsafe { $name(a, b) })
    };
    ($name:ident :: <$k:literal>, $a:ty, $b:ty) => {
        bin(concat!(stringify!($name), "::<", $k, ">"), |a: $a, b: $b| unsafe { $name::<$k>(a, b) }, |a: $a, b: $b| unsafe { $name::<$k>(a, b) })
    };
}
macro_rules! t {
    ($name:ident, $a:ty, $b:ty, $c:ty) => {
        tri(stringify!($name), |a: $a, b: $b, c: $c| unsafe { $name(a, b, c) }, |a: $a, b: $b, c: $c| unsafe { $name(a, b, c) })
    };
}
macro_rules! fma {
    ($name:ident, $v:ty, $e:ty, $l:literal) => {
        fma::<$v, $e, $l>(stringify!($name), |a: $v, b: $v, c: $v| unsafe { $name(a, b, c) }, |a: $v, b: $v, c: $v| unsafe { $name(a, b, c) })
    };
}

#[cfg(target_arch = "aarch64")]
#[inline(never)]
fn chain_adler(a: [u8; 64], b: [u8; 64]) -> [u32; 4] {
    unsafe {
            let mut s1 = vdupq_n_u32(1);
            let mut s2 = vdupq_n_u32(0);
            let mut w = vdupq_n_u16(0);
            let mut i = 0;
            while i < 64 {
                let x = vld1q_u8(a.as_ptr().add(i));
                let y = vld1q_u8(b.as_ptr().add(i));
                s2 = vaddq_u32(s2, vshlq_n_u32::<4>(s1));
                w = vpadalq_u8(w, x);
                w = vaddw_u8(w, vget_low_u8(y));
                s1 = vpadalq_u16(s1, vpaddlq_u8(x));
                s2 = vmlal_high_u16(s2, w, vpaddlq_u8(y));
                s2 = vmlal_u16(s2, vget_low_u16(w), vget_high_u16(w));
                i += 16;
            }
            [vaddvq_u32(s1), vaddvq_u32(s2), vmaxvq_u32(s2), vgetq_lane_u32::<1>(s1)]
        }
}

#[cfg(target_arch = "aarch64")]
fn arch() {
    tri("f32::mul_add", |a: f32, b: f32, c: f32| a.mul_add(b, c), |a: f32, b: f32, c: f32| a.mul_add(b, c));
    tri("f64::mul_add", |a: f64, b: f64, c: f64| a.mul_add(b, c), |a: f64, b: f64, c: f64| a.mul_add(b, c));
    b!(vaddq_s16, int16x8_t, int16x8_t);
    b!(vaddq_s32, int32x4_t, int32x4_t);
    b!(vaddq_u32, uint32x4_t, uint32x4_t);
    b!(vsubq_s16, int16x8_t, int16x8_t);
    b!(vsubq_s32, int32x4_t, int32x4_t);
    b!(vmulq_s16, int16x8_t, int16x8_t);
    b!(vmulq_s32, int32x4_t, int32x4_t);
    b!(vmaxq_s16, int16x8_t, int16x8_t);
    b!(vminq_s16, int16x8_t, int16x8_t);
    b!(vorrq_s32, int32x4_t, int32x4_t);
    b!(vzip1q_s16, int16x8_t, int16x8_t);
    b!(vzip2q_s16, int16x8_t, int16x8_t);
    b!(vtrn1q_s64, int64x2_t, int64x2_t);
    b!(vtrn2q_s64, int64x2_t, int64x2_t);
    u!(vdupq_n_u8, u8);
    u!(vdupq_n_s16, i16);
    u!(vdupq_n_u16, u16);
    u!(vdupq_n_s32, i32);
    u!(vdupq_n_u32, u32);
    u!(vgetq_lane_u32::<0>, uint32x4_t);
    u!(vgetq_lane_u32::<3>, uint32x4_t);
    u!(vget_lane_u32::<0>, uint32x2_t);
    u!(vget_lane_u32::<1>, uint32x2_t);
    b!(vsetq_lane_u32::<0>, u32, uint32x4_t);
    b!(vsetq_lane_u32::<2>, u32, uint32x4_t);
    u!(vaddvq_u32, uint32x4_t);
    u!(vmaxvq_u32, uint32x4_t);
    b!(vaddw_u8, uint16x8_t, uint8x8_t);
    b!(vaddw_high_u8, uint16x8_t, uint8x16_t);
    b!(vpaddq_u8, uint8x16_t, uint8x16_t);
    b!(vsriq_n_u8::<1>, uint8x16_t, uint8x16_t);
    b!(vsriq_n_u8::<5>, uint8x16_t, uint8x16_t);
    b!(vsriq_n_u8::<8>, uint8x16_t, uint8x16_t);
    t!(vmlal_u16, uint32x4_t, uint16x4_t, uint16x4_t);
    t!(vmlal_high_u16, uint32x4_t, uint16x8_t, uint16x8_t);
    b!(vpadalq_u8, uint16x8_t, uint8x16_t);
    b!(vpadalq_u16, uint32x4_t, uint16x8_t);
    u!(vpaddlq_u8, uint8x16_t);
    u!(vshlq_n_u32::<0>, uint32x4_t);
    u!(vshlq_n_u32::<5>, uint32x4_t);
    u!(vshlq_n_u32::<31>, uint32x4_t);
    u!(vqshlq_n_u32::<0>, uint32x4_t);
    u!(vqshlq_n_u32::<6>, uint32x4_t);
    u!(vqshlq_n_u32::<31>, uint32x4_t);
    u!(vshrq_n_s16::<1>, int16x8_t);
    u!(vshrq_n_s16::<16>, int16x8_t);
    u!(vshrq_n_s32::<7>, int32x4_t);
    u!(vshrq_n_s32::<32>, int32x4_t);
    u!(vget_low_u8, uint8x16_t);
    u!(vget_low_u16, uint16x8_t);
    u!(vget_low_u32, uint32x4_t);
    u!(vget_low_s16, int16x8_t);
    u!(vget_high_u8, uint8x16_t);
    u!(vget_high_u16, uint16x8_t);
    u!(vget_high_u32, uint32x4_t);
    b!(vcombine_u8, uint8x8_t, uint8x8_t);
    b!(vceq_u8, uint8x8_t, uint8x8_t);
    u!(vcltz_s8, int8x8_t);
    u!(vdup_n_u8, u8);
    u!(vreinterpret_u64_u8, uint8x8_t);
    u!(vget_lane_u64::<0>, uint64x1_t);
    un("vld1_u8", |p: [u8; 9]| unsafe { vld1_u8(p.as_ptr().add(1)) }, |p: [u8; 9]| unsafe { vld1_u8(p.as_ptr().add(1)) });
    b!(vcombine_u16, uint16x4_t, uint16x4_t);
    b!(vcombine_s16, int16x4_t, int16x4_t);
    un("vld1q_u8", |p: [u8; 17]| unsafe { vld1q_u8(p.as_ptr().add(1)) }, |p: [u8; 17]| unsafe { vld1q_u8(p.as_ptr().add(1)) });
    un("vld1q_u16", |p: [u16; 8]| unsafe { vld1q_u16(p.as_ptr()) }, |p: [u16; 8]| unsafe { vld1q_u16(p.as_ptr()) });
    un("vld1_u16", |p: [u16; 4]| unsafe { vld1_u16(p.as_ptr()) }, |p: [u16; 4]| unsafe { vld1_u16(p.as_ptr()) });
    un("vld1q_u32", |p: [u32; 4]| unsafe { vld1q_u32(p.as_ptr()) }, |p: [u32; 4]| unsafe { vld1q_u32(p.as_ptr()) });
    un("vld1q_s16", |p: [i16; 8]| unsafe { vld1q_s16(p.as_ptr()) }, |p: [i16; 8]| unsafe { vld1q_s16(p.as_ptr()) });
    un(
        "vst1q_u32",
        |a: uint32x4_t| {
            let mut o = [0u32; 6];
            unsafe { vst1q_u32(o.as_mut_ptr().add(1), a) };
            o
        },
        |a: uint32x4_t| {
            let mut o = [0u32; 6];
            unsafe { vst1q_u32(o.as_mut_ptr().add(1), a) };
            o
        },
    );
    un(
        "vst1q_s16",
        |a: int16x8_t| {
            let mut o = [0i16; 10];
            unsafe { vst1q_s16(o.as_mut_ptr().add(1), a) };
            o
        },
        |a: int16x8_t| {
            let mut o = [0i16; 10];
            unsafe { vst1q_s16(o.as_mut_ptr().add(1), a) };
            o
        },
    );
    // relax issue 1: one or two rows of each NEON family
    u!(vreinterpret_s8_f32, float32x2_t);
    u!(vreinterpretq_f64_u8, uint8x16_t);
    u!(vreinterpretq_p128_p64, poly64x2_t);
    u!(vreinterpretq_p64_p128, u128);
    un("vld2q_u8", |p: [u8; 33]| unsafe { vld2q_u8(p.as_ptr().add(1)) }, |p: [u8; 33]| unsafe { vld2q_u8(p.as_ptr().add(1)) });
    un("vld3_s16", |p: [i16; 12]| unsafe { vld3_s16(p.as_ptr()) }, |p: [i16; 12]| unsafe { vld3_s16(p.as_ptr()) });
    un("vld4q_u32", |p: [u32; 16]| unsafe { vld4q_u32(p.as_ptr()) }, |p: [u32; 16]| unsafe { vld4q_u32(p.as_ptr()) });
    un("vld1q_u8_x4", |p: [u8; 64]| unsafe { vld1q_u8_x4(p.as_ptr()) }, |p: [u8; 64]| unsafe { vld1q_u8_x4(p.as_ptr()) });
    un("vld1q_dup_u16", |p: u16| unsafe { vld1q_dup_u16(&p) }, |p: u16| unsafe { vld1q_dup_u16(&p) });
    un("vld1q_lane_u8::<5>", |(x, v): (u8, uint8x16_t)| unsafe { vld1q_lane_u8::<5>(&x, v) }, |(x, v): (u8, uint8x16_t)| unsafe { vld1q_lane_u8::<5>(&x, v) });
    un(
        "vst3q_u8",
        |a: uint8x16x3_t| {
            let mut o = [0u8; 50];
            unsafe { vst3q_u8(o.as_mut_ptr().add(1), a) };
            o
        },
        |a: uint8x16x3_t| {
            let mut o = [0u8; 50];
            unsafe { vst3q_u8(o.as_mut_ptr().add(1), a) };
            o
        },
    );
    un(
        "vst1_lane_s32::<1>",
        |a: int32x2_t| {
            let mut o = 0i32;
            unsafe { vst1_lane_s32::<1>(&mut o, a) };
            o
        },
        |a: int32x2_t| {
            let mut o = 0i32;
            unsafe { vst1_lane_s32::<1>(&mut o, a) };
            o
        },
    );
    u!(vdup_n_s16, i16);
    u!(vmovq_n_u8, u8);
    u!(vcreate_u16, u64);
    b!(vcombine_f32, float32x2_t, float32x2_t);
    u!(vget_high_p64, poly64x2_t);
    u!(vget_low_f32, float32x4_t);
    u!(vgetq_lane_p8::<15>, poly8x16_t);
    u!(vget_lane_f64::<0>, float64x1_t);
    b!(vsetq_lane_p64::<1>, u64, poly64x2_t);
    u!(vrev64q_u8, uint8x16_t);
    u!(vrev32_u16, uint16x4_t);
    u!(vrev16q_u8, uint8x16_t);
    b!(vadd_u8, uint8x8_t, uint8x8_t);
    b!(vaddq_p64, poly64x2_t, poly64x2_t);
    b!(vsub_s64, int64x1_t, int64x1_t);
    b!(vmul_f32, float32x2_t, float32x2_t);
    b!(vmulq_u8, uint8x16_t, uint8x16_t);
    b!(vand_u64, uint64x1_t, uint64x1_t);
    b!(vorr_u16, uint16x4_t, uint16x4_t);
    b!(veor_s8, int8x8_t, int8x8_t);
    b!(vqadd_s8, int8x8_t, int8x8_t);
    b!(vqaddq_u64, uint64x2_t, uint64x2_t);
    b!(vqsub_u16, uint16x4_t, uint16x4_t);
    b!(vqsubq_s64, int64x2_t, int64x2_t);
    b!(vmax_s8, int8x8_t, int8x8_t);
    b!(vmin_u32, uint32x2_t, uint32x2_t);
    b!(vbic_u8, uint8x8_t, uint8x8_t);
    b!(vornq_s32, int32x4_t, int32x4_t);
    u!(vmvnq_p8, poly8x16_t);
    u!(vneg_s8, int8x8_t);
    u!(vnegq_f64, float64x2_t);
    u!(vabs_s16, int16x4_t);
    u!(vabs_f32, float32x2_t);
    t!(vmla_s8, int8x8_t, int8x8_t, int8x8_t);
    t!(vmlsq_u32, uint32x4_t, uint32x4_t, uint32x4_t);
    t!(vmla_f32, float32x2_t, float32x2_t, float32x2_t);
    b!(vmul_n_u16, uint16x4_t, u16);
    b!(vmul_n_f64, float64x1_t, f64);
    t!(vmlaq_n_u32, uint32x4_t, uint32x4_t, u32);
    t!(vmls_n_f32, float32x2_t, float32x2_t, f32);
    b!(vqdmulh_s16, int16x4_t, int16x4_t);
    b!(vqdmulhq_s32, int32x4_t, int32x4_t);
    b!(vqrdmulh_s16, int16x4_t, int16x4_t);
    b!(vqrdmulhq_s32, int32x4_t, int32x4_t);
    b!(vqdmulhq_n_s16, int16x8_t, i16);
    b!(vqrdmulh_n_s32, int32x2_t, i32);
    u!(vshl_n_u8::<3>, uint8x8_t);
    u!(vshr_n_s16::<16>, int16x4_t);
    u!(vshr_n_u32::<32>, uint32x2_t);
    u!(vshr_n_u64::<1>, uint64x1_t);
    u!(vshll_n_s8::<8>, int8x8_t);
    u!(vshll_n_u16::<3>, uint16x4_t);
    u!(vshrn_n_s16::<8>, int16x8_t);
    u!(vshrn_n_u32::<16>, uint32x4_t);
    u!(vmovn_u64, uint64x2_t);
    u!(vmovn_s16, int16x8_t);
    b!(vceq_f32, float32x2_t, float32x2_t);
    b!(vcge_s64, int64x1_t, int64x1_t);
    b!(vcgt_u8, uint8x8_t, uint8x8_t);
    b!(vcle_f64, float64x1_t, float64x1_t);
    b!(vclt_s16, int16x4_t, int16x4_t);
    u!(vceqzq_f64, float64x2_t);
    u!(vcgezq_s8, int8x16_t);
    u!(vcgtz_s32, int32x2_t);
    u!(vclezq_f32, float32x4_t);
    u!(vcltzq_s64, int64x2_t);
    b!(vtst_u16, uint16x4_t, uint16x4_t);
    b!(vtstq_p64, poly64x2_t, poly64x2_t);
    b!(__crc32h, u32, u16);
    b!(__crc32w, u32, u32);
    b!(vmull_p64, u64, u64);
    b!(vmull_high_p64, poly64x2_t, poly64x2_t);
    b!(vmul_p8, poly8x8_t, poly8x8_t);
    b!(vmulq_p8, poly8x16_t, poly8x16_t);
    b!(vmull_p8, poly8x8_t, poly8x8_t);
    b!(vmull_high_p8, poly8x16_t, poly8x16_t);
    // a chain in a loop: adler32-style sums (vector values across iterations)
    bin(
        "chain_adler",
        |a: [u8; 64], b: [u8; 64]| chain_adler(a, b),
        |a: [u8; 64], b: [u8; 64]| {
            let mut h = 0u32;
            for i in 0..64 {
                h = h.wrapping_mul(31).wrapping_add(a[i] as u32 ^ b[i] as u32);
            }
            [h, 0, 0, 0]
        },
    );
}

#[cfg(target_arch = "x86_64")]
#[inline(never)]
fn chain_adler(a: [u8; 64], b: [u8; 64]) -> [i32; 4] {
    unsafe {
            let ones = _mm256_set1_epi16(1);
            let mut s1 = _mm256_setzero_si256();
            let mut s2 = _mm256_setzero_si256();
            let mut i = 0;
            while i < 64 {
                let x = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
                let y = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
                s2 = _mm256_add_epi32(s2, _mm256_slli_epi32::<5>(s1));
                s1 = _mm256_add_epi32(s1, _mm256_sad_epu8(x, _mm256_setzero_si256()));
                let m = _mm256_maddubs_epi16(y, x);
                s2 = _mm256_add_epi32(s2, _mm256_madd_epi16(m, ones));
                i += 32;
            }
            let lo = _mm256_castsi256_si128(s2);
            let hi = _mm256_extracti128_si256::<1>(s2);
            let t = _mm_add_epi32(lo, hi);
            let t = _mm_add_epi32(t, _mm_shuffle_epi32::<0x4e>(t));
            let t = _mm_add_epi32(t, _mm_srli_si128::<4>(t));
            [_mm_cvtsi128_si32(t), _mm_extract_epi32::<2>(_mm256_castsi256_si128(s1)), 0, 0]
        }
}

#[cfg(target_arch = "x86_64")]
fn arch() {
    b!(_mm_add_epi8, __m128i, __m128i);
    b!(_mm_add_epi16, __m128i, __m128i);
    b!(_mm_add_epi32, __m128i, __m128i);
    b!(_mm_sub_epi8, __m128i, __m128i);
    b!(_mm_sub_epi16, __m128i, __m128i);
    b!(_mm_and_si128, __m128i, __m128i);
    b!(_mm_or_si128, __m128i, __m128i);
    b!(_mm_xor_si128, __m128i, __m128i);
    b!(_mm_andnot_si128, __m128i, __m128i);
    b!(_mm_avg_epu8, __m128i, __m128i);
    b!(_mm_cmpeq_epi16, __m128i, __m128i);
    b!(_mm_cmpeq_epi8, __m128i, __m128i);
    b!(_mm_cmpgt_epi8, __m128i, __m128i);
    u!(_mm_movemask_epi8, __m128i);
    un(
        "_mm_store_si128",
        |a: __m128i| {
            let mut o = [a, a];
            unsafe { _mm_store_si128(&mut o[1], _mm_setzero_si128()) };
            o
        },
        |a: __m128i| {
            let mut o = [a, a];
            unsafe { _mm_store_si128(&mut o[1], _mm_setzero_si128()) };
            o
        },
    );
    b!(_mm_min_epi16, __m128i, __m128i);
    b!(_mm_madd_epi16, __m128i, __m128i);
    b!(_mm_maddubs_epi16, __m128i, __m128i);
    b!(_mm_sad_epu8, __m128i, __m128i);
    b!(_mm_packus_epi16, __m128i, __m128i);
    b!(_mm_unpacklo_epi8, __m128i, __m128i);
    b!(_mm_unpackhi_epi8, __m128i, __m128i);
    b!(_mm_unpackhi_epi64, __m128i, __m128i);
    b!(_mm_shuffle_epi8, __m128i, __m128i);
    u!(_mm_abs_epi16, __m128i);
    u!(_mm_shuffle_epi32::<0>, __m128i);
    u!(_mm_shuffle_epi32::<27>, __m128i);
    u!(_mm_shuffle_epi32::<177>, __m128i);
    u!(_mm_shuffle_epi32::<255>, __m128i);
    u!(_mm_slli_epi32::<0>, __m128i);
    u!(_mm_slli_epi32::<9>, __m128i);
    u!(_mm_slli_epi32::<31>, __m128i);
    u!(_mm_slli_epi32::<32>, __m128i);
    u!(_mm_srli_si128::<0>, __m128i);
    u!(_mm_srli_si128::<4>, __m128i);
    u!(_mm_srli_si128::<8>, __m128i);
    u!(_mm_srli_si128::<15>, __m128i);
    u!(_mm_srli_si128::<16>, __m128i);
    u!(_mm_set1_epi8, i8);
    u!(_mm_set1_epi16, i16);
    un("_mm_setzero_si128", |_a: u8| unsafe { _mm_setzero_si128() }, |_a: u8| unsafe { _mm_setzero_si128() });
    b!(_mm_set_epi64x, i64, i64);
    u!(_mm_cvtsi32_si128, i32);
    u!(_mm_cvtsi128_si32, __m128i);
    u!(_mm_extract_epi32::<0>, __m128i);
    u!(_mm_extract_epi32::<3>, __m128i);
    un("_mm_loadu_si128", |p: [u8; 17]| unsafe { _mm_loadu_si128(p.as_ptr().add(1) as *const __m128i) }, |p: [u8; 17]| unsafe { _mm_loadu_si128(p.as_ptr().add(1) as *const __m128i) });
    un("_mm_load_si128", |p: __m128i| unsafe { _mm_load_si128(&p) }, |p: __m128i| unsafe { _mm_load_si128(&p) });
    un(
        "_mm_storeu_si128",
        |a: __m128i| {
            let mut o = [0u8; 20];
            unsafe { _mm_storeu_si128(o.as_mut_ptr().add(3) as *mut __m128i, a) };
            o
        },
        |a: __m128i| {
            let mut o = [0u8; 20];
            unsafe { _mm_storeu_si128(o.as_mut_ptr().add(3) as *mut __m128i, a) };
            o
        },
    );
    b!(_mm256_add_epi16, __m256i, __m256i);
    b!(_mm256_add_epi32, __m256i, __m256i);
    b!(_mm256_sub_epi16, __m256i, __m256i);
    b!(_mm256_sub_epi32, __m256i, __m256i);
    b!(_mm256_mullo_epi16, __m256i, __m256i);
    b!(_mm256_mullo_epi32, __m256i, __m256i);
    b!(_mm256_max_epi16, __m256i, __m256i);
    b!(_mm256_min_epi16, __m256i, __m256i);
    b!(_mm256_or_si256, __m256i, __m256i);
    b!(_mm256_madd_epi16, __m256i, __m256i);
    b!(_mm256_maddubs_epi16, __m256i, __m256i);
    b!(_mm256_sad_epu8, __m256i, __m256i);
    b!(_mm256_packus_epi16, __m256i, __m256i);
    b!(_mm256_packus_epi32, __m256i, __m256i);
    b!(_mm256_packs_epi32, __m256i, __m256i);
    b!(_mm256_shuffle_epi8, __m256i, __m256i);
    b!(_mm256_unpacklo_epi8, __m256i, __m256i);
    b!(_mm256_unpackhi_epi8, __m256i, __m256i);
    b!(_mm256_unpacklo_epi16, __m256i, __m256i);
    b!(_mm256_unpackhi_epi16, __m256i, __m256i);
    b!(_mm256_unpacklo_epi32, __m256i, __m256i);
    b!(_mm256_unpackhi_epi32, __m256i, __m256i);
    b!(_mm256_unpacklo_epi64, __m256i, __m256i);
    b!(_mm256_unpackhi_epi64, __m256i, __m256i);
    // 256-bit and fused float operations (two 128-bit halves in Relax)
    b!(_mm_shuffle_ps::<0>, __m128, __m128);
    b!(_mm_shuffle_ps::<0x1b>, __m128, __m128);
    b!(_mm_shuffle_ps::<0xb1>, __m128, __m128);
    b!(_mm_shuffle_ps::<0x4e>, __m128, __m128);
    b!(_mm_shuffle_ps::<0xff>, __m128, __m128);
    b!(_mm_movehl_ps, __m128, __m128);
    b!(_mm_unpacklo_ps, __m128, __m128);
    b!(_mm_unpackhi_ps, __m128, __m128);
    b!(_mm256_add_ps, __m256, __m256);
    b!(_mm256_sub_ps, __m256, __m256);
    b!(_mm256_mul_ps, __m256, __m256);
    b!(_mm256_div_ps, __m256, __m256);
    b!(_mm256_max_ps, __m256, __m256);
    b!(_mm256_min_ps, __m256, __m256);
    b!(_mm256_and_ps, __m256, __m256);
    b!(_mm256_or_ps, __m256, __m256);
    b!(_mm256_xor_ps, __m256, __m256);
    b!(_mm256_andnot_ps, __m256, __m256);
    fma!(_mm256_fmadd_ps, __m256, u32, 8);
    fma!(_mm256_fmsub_ps, __m256, u32, 8);
    fma!(_mm256_fnmadd_ps, __m256, u32, 8);
    fma!(_mm256_fnmsub_ps, __m256, u32, 8);
    fma!(_mm_fmadd_ps, __m128, u32, 4);
    fma!(_mm_fmsub_ps, __m128, u32, 4);
    fma!(_mm_fnmadd_ps, __m128, u32, 4);
    fma!(_mm_fnmsub_ps, __m128, u32, 4);
    fma!(_mm_fmadd_pd, __m128d, u64, 2);
    fma!(_mm_fmsub_pd, __m128d, u64, 2);
    fma!(_mm_fnmadd_pd, __m128d, u64, 2);
    fma!(_mm_fnmsub_pd, __m128d, u64, 2);
    fma!(_mm_fmaddsub_pd, __m128d, u64, 2);
    fma!(_mm_fmsubadd_pd, __m128d, u64, 2);
    fma!(_mm_fmadd_sd, __m128d, u64, 2);
    fma!(_mm_fmsub_sd, __m128d, u64, 2);
    fma!(_mm_fnmadd_sd, __m128d, u64, 2);
    fma!(_mm_fnmsub_sd, __m128d, u64, 2);
    fma!(_mm256_fmadd_pd, __m256d, u64, 4);
    fma!(_mm256_fmsub_pd, __m256d, u64, 4);
    fma!(_mm256_fnmadd_pd, __m256d, u64, 4);
    fma!(_mm256_fnmsub_pd, __m256d, u64, 4);
    fma!(_mm256_fmaddsub_pd, __m256d, u64, 4);
    fma!(_mm256_fmsubadd_pd, __m256d, u64, 4);
    b!(_mm256_and_si256, __m256i, __m256i);
    b!(_mm256_cmpeq_epi8, __m256i, __m256i);
    u!(_mm256_set1_ps, f32);
    un("_mm256_setzero_ps", |_a: u8| unsafe { _mm256_setzero_ps() }, |_a: u8| unsafe { _mm256_setzero_ps() });
    u!(_mm256_castps256_ps128, __m256);
    u!(_mm256_extractf128_ps::<0>, __m256);
    u!(_mm256_extractf128_ps::<1>, __m256);
    un("_mm256_loadu_ps", |p: [u8; 40]| unsafe { _mm256_loadu_ps(p.as_ptr().add(5) as *const f32) }, |p: [u8; 40]| unsafe { _mm256_loadu_ps(p.as_ptr().add(5) as *const f32) });
    un(
        "_mm256_storeu_ps",
        |a: __m256| {
            let mut o = [0u8; 40];
            unsafe { _mm256_storeu_ps(o.as_mut_ptr().add(5) as *mut f32, a) };
            o
        },
        |a: __m256| {
            let mut o = [0u8; 40];
            unsafe { _mm256_storeu_ps(o.as_mut_ptr().add(5) as *mut f32, a) };
            o
        },
    );
    // scalar fused multiply-add (f32::mul_add / f64::mul_add)
    tri("f32::mul_add", |a: f32, b: f32, c: f32| a.mul_add(b, c), |a: f32, b: f32, c: f32| a.mul_add(b, c));
    tri("f64::mul_add", |a: f64, b: f64, c: f64| a.mul_add(b, c), |a: f64, b: f64, c: f64| a.mul_add(b, c));
    u!(_mm256_set1_epi16, i16);
    u!(_mm256_set1_epi32, i32);
    un("_mm256_setzero_si256", |_a: u8| unsafe { _mm256_setzero_si256() }, |_a: u8| unsafe { _mm256_setzero_si256() });
    u!(_mm256_slli_epi16::<0>, __m256i);
    u!(_mm256_slli_epi16::<3>, __m256i);
    u!(_mm256_slli_epi16::<16>, __m256i);
    u!(_mm256_slli_epi32::<17>, __m256i);
    u!(_mm256_srai_epi16::<1>, __m256i);
    u!(_mm256_srai_epi16::<15>, __m256i);
    u!(_mm256_srai_epi16::<16>, __m256i);
    u!(_mm256_srai_epi32::<31>, __m256i);
    u!(_mm256_srai_epi32::<40>, __m256i);
    u!(_mm256_castsi256_si128, __m256i);
    u!(_mm256_extracti128_si256::<0>, __m256i);
    u!(_mm256_extracti128_si256::<1>, __m256i);
    u!(_mm256_extractf128_si256::<1>, __m256i);
    un("_mm256_loadu_si256", |p: [u8; 40]| unsafe { _mm256_loadu_si256(p.as_ptr().add(5) as *const __m256i) }, |p: [u8; 40]| unsafe { _mm256_loadu_si256(p.as_ptr().add(5) as *const __m256i) });
    un(
        "_mm256_storeu_si256",
        |a: __m256i| {
            let mut o = [0u8; 40];
            unsafe { _mm256_storeu_si256(o.as_mut_ptr().add(5) as *mut __m256i, a) };
            o
        },
        |a: __m256i| {
            let mut o = [0u8; 40];
            unsafe { _mm256_storeu_si256(o.as_mut_ptr().add(5) as *mut __m256i, a) };
            o
        },
    );
    // a chain in a loop: adler32-style sums (vector values across iterations)
    bin(
        "chain_adler",
        |a: [u8; 64], b: [u8; 64]| chain_adler(a, b),
        |a: [u8; 64], b: [u8; 64]| {
            let mut h = 0i32;
            for i in 0..64 {
                h = h.wrapping_mul(31).wrapping_add((a[i] ^ b[i]) as i32);
            }
            [h, 0, 0, 0]
        },
    );
}

#[test]
fn simd_arch() {
    arch();
}

// relax issue 1: sixteen decimal digits of a u64 with NEON lane arithmetic, as a float formatter
// splits its digits (multiply-high instead of division, interleaving stores), against format!
#[cfg(target_arch = "aarch64")]
fn digits16(v: u64) -> [u8; 16] {
    unsafe {
        let (hi, lo) = ((v / 100_000_000) as u32, (v % 100_000_000) as u32);
        let groups = [(hi / 10_000) as u16, (hi % 10_000) as u16, (lo / 10_000) as u16, (lo % 10_000) as u16];
        let g = vld1_u16(groups.as_ptr());
        // 4 digits -> 2 + 2: g / 100 is (g * 5243) >> 19, as a doubling multiply-high then >> 4, and
        // widened to 32 bits, narrowed by >> 16, then >> 3
        let q = vreinterpret_u16_s16(vshr_n_s16::<4>(vqdmulh_n_s16(vreinterpret_s16_u16(g), 5243)));
        let q32 = vshr_n_u16::<3>(vshrn_n_u32::<16>(vmulq_n_u32(vshll_n_u16::<0>(g), 5243)));
        assert_eq!(vget_lane_u64::<0>(vreinterpret_u64_u16(vceq_u16(q, q32))), u64::MAX, "two ways to g / 100 differ for {v}");
        let r = vmls_n_u16(g, q, 100);
        let mut pairs = [0u16; 8];
        vst2_u16(pairs.as_mut_ptr(), uint16x4x2_t(q, r));
        // 2 digits -> tens and units: p / 10 is (p * 103) >> 10
        let p = vld1q_u16(pairs.as_ptr());
        let tens = vshrq_n_u16::<10>(vmulq_n_u16(p, 103));
        let units = vmlsq_n_u16(p, tens, 10);
        let zero = vdup_n_u8(b'0');
        let mut out = [0u8; 16];
        vst2_u8(out.as_mut_ptr(), uint8x8x2_t(vadd_u8(vmovn_u16(tens), zero), vadd_u8(vmovn_u16(units), zero)));
        out
    }
}

#[cfg(target_arch = "aarch64")]
#[test]
fn neon_decimal_digits() {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut values = vec![0, 1, 9, 10, 99, 100, 9_999, 10_000, 99_999_999, 100_000_000, 1_000_000_000_000_000, 9_999_999_999_999_999];
    for _ in 0..200 {
        values.push(next(&mut s) % 10_000_000_000_000_000);
    }
    for v in values {
        let d = digits16(core::hint::black_box(v));
        assert_eq!(core::str::from_utf8(&d).unwrap(), format!("{v:016}"), "digits of {v}");
    }
    println!("neon decimal digits: ok");
}

// relax known gap 7: inside a function with the target features, the NEON, CRC32 and PMULL
// intrinsics are safe to call (no `unsafe` block), as rustc's are
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon,aes,crc")]
fn safe_in_target_feature_fn(x: u8, c: u32) -> (u64, u32, u128, u32) {
    let a = vdup_n_u8(x);
    let b = vadd_u8(a, vshl_n_u8::<1>(a));
    let q = vaddq_u32(vdupq_n_u32(c), vdupq_n_u32(1));
    (vget_lane_u64::<0>(vreinterpret_u64_u8(b)), __crc32b(c, x), vmull_p64(c as u64, 3), vgetq_lane_u32::<2>(q))
}

#[cfg(target_arch = "aarch64")]
#[test]
fn neon_safe_in_target_feature_fn() {
    if !(std::arch::is_aarch64_feature_detected!("aes") && std::arch::is_aarch64_feature_detected!("crc")) {
        return;
    }
    let (lanes, crc, product, lane) = unsafe { safe_in_target_feature_fn(core::hint::black_box(3), 7) };
    let mut r = 7u32 ^ 3;
    for _ in 0..8 {
        r = (r >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(r & 1));
    }
    assert_eq!((lanes, crc, product, lane), (0x0909_0909_0909_0909, r, 9, 8), "vdup/vshl/vadd, __crc32b, vmull_p64, vaddq/vgetq_lane");
    println!("neon safe in target_feature fn: ok");
}
