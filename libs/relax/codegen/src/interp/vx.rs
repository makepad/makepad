//! Lane-typed vector ops (`Inst::VX`, `Inst::VX3`) for the threaded interpreter: the same NEON
//! instructions the arm64 backend emits for each op (arm64.rs `vx` / `vx3`), through core::arch,
//! so results are bit-identical, including the SHA instruction sequences that give Intel's
//! semantics on Arm.

use super::{c, hist_note, Cx, COUNT, H};
use relax_backend::rir::{Cond, NanBits, RoundTo, VW, VX};
use core::arch::aarch64::*;

#[inline(always)]
unsafe fn ld(p: *const u64) -> uint8x16_t {
    vld1q_u8(p as *const u8)
}
#[inline(always)]
unsafe fn st(p: *mut u64, v: uint8x16_t) {
    vst1q_u8(p as *mut u8, v)
}

macro_rules! by_w {
    ($w:expr, $a:expr, $b:expr, $u8:ident, $u16:ident, $u32:ident, $u64:ident) => {
        match $w {
            VW::B => $u8($a, $b),
            VW::H => vreinterpretq_u8_u16($u16(vreinterpretq_u16_u8($a), vreinterpretq_u16_u8($b))),
            VW::S => vreinterpretq_u8_u32($u32(vreinterpretq_u32_u8($a), vreinterpretq_u32_u8($b))),
            VW::D => vreinterpretq_u8_u64($u64(vreinterpretq_u64_u8($a), vreinterpretq_u64_u8($b))),
        }
    };
}
macro_rules! by_ws {
    ($w:expr, $a:expr, $b:expr, $s8:ident, $s16:ident, $s32:ident, $s64:ident) => {
        match $w {
            VW::B => vreinterpretq_u8_s8($s8(vreinterpretq_s8_u8($a), vreinterpretq_s8_u8($b))),
            VW::H => vreinterpretq_u8_s16($s16(vreinterpretq_s16_u8($a), vreinterpretq_s16_u8($b))),
            VW::S => vreinterpretq_u8_s32($s32(vreinterpretq_s32_u8($a), vreinterpretq_s32_u8($b))),
            VW::D => vreinterpretq_u8_s64($s64(vreinterpretq_s64_u8($a), vreinterpretq_s64_u8($b))),
        }
    };
}
macro_rules! by_wc {
    ($w:expr, $a:expr, $b:expr, $s8:ident, $s16:ident, $s32:ident, $s64:ident) => {
        match $w {
            VW::B => $s8(vreinterpretq_s8_u8($a), vreinterpretq_s8_u8($b)),
            VW::H => vreinterpretq_u8_u16($s16(vreinterpretq_s16_u8($a), vreinterpretq_s16_u8($b))),
            VW::S => vreinterpretq_u8_u32($s32(vreinterpretq_s32_u8($a), vreinterpretq_s32_u8($b))),
            VW::D => vreinterpretq_u8_u64($s64(vreinterpretq_s64_u8($a), vreinterpretq_s64_u8($b))),
        }
    };
}

/// Per-lane operation on lanes of width w as u64 values (ops NEON has no 64-bit form of, and
/// immediate shifts).
#[inline(always)]
unsafe fn lanes(w: VW, a: uint8x16_t, b: uint8x16_t, f: impl Fn(u64, u64) -> u64) -> uint8x16_t {
    let (mut x, mut y) = ([0u8; 16], [0u8; 16]);
    vst1q_u8(x.as_mut_ptr(), a);
    vst1q_u8(y.as_mut_ptr(), b);
    let n = (w.bits() / 8) as usize;
    let mut r = [0u8; 16];
    for i in 0..16 / n {
        let mut p = [0u8; 8];
        let mut q = [0u8; 8];
        p[..n].copy_from_slice(&x[i * n..i * n + n]);
        q[..n].copy_from_slice(&y[i * n..i * n + n]);
        let v = f(u64::from_le_bytes(p), u64::from_le_bytes(q));
        r[i * n..i * n + n].copy_from_slice(&v.to_le_bytes()[..n]);
    }
    vld1q_u8(r.as_ptr())
}

#[inline(always)]
fn sext(v: u64, bits: u32) -> i64 {
    ((v << (64 - bits)) as i64) >> (64 - bits)
}

/// Reverse the four 32-bit words (rev64 .4s; ext #8).
#[inline(always)]
unsafe fn rev4(v: uint32x4_t) -> uint32x4_t {
    let r = vrev64q_u32(v);
    vextq_u32::<2>(r, r)
}

#[inline(always)]
unsafe fn u32x(v: uint8x16_t) -> uint32x4_t {
    vreinterpretq_u32_u8(v)
}
#[inline(always)]
unsafe fn u8x(v: uint32x4_t) -> uint8x16_t {
    vreinterpretq_u8_u32(v)
}

/// All-ones lanes where `v` is a NaN (lanes of width w: S or D).
#[inline(always)]
unsafe fn is_nan(w: VW, v: uint8x16_t) -> uint8x16_t {
    match w {
        VW::D => vmvnq_u8(vreinterpretq_u8_u64(vceqq_f64(vreinterpretq_f64_u8(v), vreinterpretq_f64_u8(v)))),
        _ => vmvnq_u8(vreinterpretq_u8_u32(vceqq_f32(vreinterpretq_f32_u8(v), vreinterpretq_f32_u8(v)))),
    }
}

/// SSE's NaN rule for a float lane op (arm64.rs `intel_nan`): a NaN result lane becomes the
/// negative default NaN, then a NaN lane of `b`, then of `a` (each quieted) takes its place.
#[inline(always)]
unsafe fn intel_nan_v(w: VW, r: uint8x16_t, a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let (dflt, quiet) = match w {
        VW::D => (vreinterpretq_u8_u64(vdupq_n_u64(0xfff8_0000_0000_0000)), vreinterpretq_u8_u64(vdupq_n_u64(0x0008_0000_0000_0000))),
        _ => (vreinterpretq_u8_u32(vdupq_n_u32(0xffc0_0000)), vreinterpretq_u8_u32(vdupq_n_u32(0x0040_0000))),
    };
    let mut r = vbslq_u8(is_nan(w, r), dflt, r);
    for input in [b, a] {
        r = vbslq_u8(is_nan(w, input), vorrq_u8(input, quiet), r);
    }
    r
}

/// The lane-wise float, compare, shift, negate and conversion ops at every width the arm64
/// back end emits them for (arm64.rs `vx_lanes`), with the instructions it emits, so the bits
/// match. None: not one of these.
#[inline(always)]
unsafe fn lane_op(op: VX, x: uint8x16_t, y: uint8x16_t) -> Option<uint8x16_t> {
    macro_rules! f2 {
        ($w:expr, $f32:ident, $f64:ident) => {
            match $w {
                VW::D => vreinterpretq_u8_f64($f64(vreinterpretq_f64_u8(x), vreinterpretq_f64_u8(y))),
                _ => vreinterpretq_u8_f32($f32(vreinterpretq_f32_u8(x), vreinterpretq_f32_u8(y))),
            }
        };
    }
    macro_rules! fc {
        ($w:expr, $f32:ident, $f64:ident, $p:expr, $q:expr) => {
            match $w {
                VW::D => vreinterpretq_u8_u64($f64(vreinterpretq_f64_u8($p), vreinterpretq_f64_u8($q))),
                _ => vreinterpretq_u8_u32($f32(vreinterpretq_f32_u8($p), vreinterpretq_f32_u8($q))),
            }
        };
    }
    macro_rules! f1 {
        ($w:expr, $f32:ident, $f64:ident) => {
            match $w {
                VW::D => vreinterpretq_u8_f64($f64(vreinterpretq_f64_u8(x))),
                _ => vreinterpretq_u8_f32($f32(vreinterpretq_f32_u8(x))),
            }
        };
    }
    // integer compares, signed (cmgt/cmge) or unsigned (cmhi/cmhs), of p and q
    macro_rules! ic {
        ($w:expr, $p:expr, $q:expr, $s8:ident, $s16:ident, $s32:ident, $s64:ident, $u8:ident, $u16:ident, $u32:ident, $u64:ident, $signed:expr) => {
            if $signed {
                match $w {
                    VW::B => $s8(vreinterpretq_s8_u8($p), vreinterpretq_s8_u8($q)),
                    VW::H => vreinterpretq_u8_u16($s16(vreinterpretq_s16_u8($p), vreinterpretq_s16_u8($q))),
                    VW::S => vreinterpretq_u8_u32($s32(vreinterpretq_s32_u8($p), vreinterpretq_s32_u8($q))),
                    VW::D => vreinterpretq_u8_u64($s64(vreinterpretq_s64_u8($p), vreinterpretq_s64_u8($q))),
                }
            } else {
                match $w {
                    VW::B => $u8($p, $q),
                    VW::H => vreinterpretq_u8_u16($u16(vreinterpretq_u16_u8($p), vreinterpretq_u16_u8($q))),
                    VW::S => vreinterpretq_u8_u32($u32(vreinterpretq_u32_u8($p), vreinterpretq_u32_u8($q))),
                    VW::D => vreinterpretq_u8_u64($u64(vreinterpretq_u64_u8($p), vreinterpretq_u64_u8($q))),
                }
            }
        };
    }
    // ushl / sshl of p by the signed per-lane amounts in q
    macro_rules! sh {
        ($w:expr, $p:expr, $q:expr, $signed:expr) => {
            match ($w, $signed) {
                (VW::B, false) => vshlq_u8($p, vreinterpretq_s8_u8($q)),
                (VW::H, false) => vreinterpretq_u8_u16(vshlq_u16(vreinterpretq_u16_u8($p), vreinterpretq_s16_u8($q))),
                (VW::S, false) => vreinterpretq_u8_u32(vshlq_u32(vreinterpretq_u32_u8($p), vreinterpretq_s32_u8($q))),
                (VW::D, false) => vreinterpretq_u8_u64(vshlq_u64(vreinterpretq_u64_u8($p), vreinterpretq_s64_u8($q))),
                (VW::B, true) => vreinterpretq_u8_s8(vshlq_s8(vreinterpretq_s8_u8($p), vreinterpretq_s8_u8($q))),
                (VW::H, true) => vreinterpretq_u8_s16(vshlq_s16(vreinterpretq_s16_u8($p), vreinterpretq_s16_u8($q))),
                (VW::S, true) => vreinterpretq_u8_s32(vshlq_s32(vreinterpretq_s32_u8($p), vreinterpretq_s32_u8($q))),
                (VW::D, true) => vreinterpretq_u8_s64(vshlq_s64(vreinterpretq_s64_u8($p), vreinterpretq_s64_u8($q))),
            }
        };
    }
    let neg = |w: VW, v: uint8x16_t| match w {
        VW::B => vreinterpretq_u8_s8(vnegq_s8(vreinterpretq_s8_u8(v))),
        VW::H => vreinterpretq_u8_s16(vnegq_s16(vreinterpretq_s16_u8(v))),
        VW::S => vreinterpretq_u8_s32(vnegq_s32(vreinterpretq_s32_u8(v))),
        VW::D => vreinterpretq_u8_s64(vnegq_s64(vreinterpretq_s64_u8(v))),
    };
    let float = |w: VW| matches!(w, VW::S | VW::D);
    Some(match op {
        VX::FAdd(w, n) if float(w) => {
            let r = f2!(w, vaddq_f32, vaddq_f64);
            if n == NanBits::Intel { intel_nan_v(w, r, x, y) } else { r }
        }
        VX::FSub(w, n) if float(w) => {
            let r = f2!(w, vsubq_f32, vsubq_f64);
            if n == NanBits::Intel { intel_nan_v(w, r, x, y) } else { r }
        }
        VX::FMul(w, n) if float(w) => {
            let r = f2!(w, vmulq_f32, vmulq_f64);
            if n == NanBits::Intel { intel_nan_v(w, r, x, y) } else { r }
        }
        VX::FDiv(w, n) if float(w) => {
            let r = f2!(w, vdivq_f32, vdivq_f64);
            if n == NanBits::Intel { intel_nan_v(w, r, x, y) } else { r }
        }
        VX::FMinNeon(w) if float(w) => f2!(w, vminq_f32, vminq_f64),
        VX::FMaxNeon(w) if float(w) => f2!(w, vmaxq_f32, vmaxq_f64),
        // b > a (min) / a > b (max), false for NaN lanes: b there
        VX::FMinLt(w) if float(w) => vbslq_u8(fc!(w, vcgtq_f32, vcgtq_f64, y, x), x, y),
        VX::FMaxGt(w) if float(w) => vbslq_u8(fc!(w, vcgtq_f32, vcgtq_f64, x, y), x, y),
        VX::FMinNum(w) | VX::FMaxNum(w) if float(w) => {
            // both inputs quieted first (fminnm x, x, x), as the back end does
            let min = matches!(op, VX::FMinNum(_));
            match w {
                VW::D => {
                    let (p, q) = (vreinterpretq_f64_u8(x), vreinterpretq_f64_u8(y));
                    let (p, q) = (vminnmq_f64(p, p), vminnmq_f64(q, q));
                    vreinterpretq_u8_f64(if min { vminnmq_f64(p, q) } else { vmaxnmq_f64(p, q) })
                }
                _ => {
                    let (p, q) = (vreinterpretq_f32_u8(x), vreinterpretq_f32_u8(y));
                    let (p, q) = (vminnmq_f32(p, p), vminnmq_f32(q, q));
                    vreinterpretq_u8_f32(if min { vminnmq_f32(p, q) } else { vmaxnmq_f32(p, q) })
                }
            }
        }
        VX::FCmp(c, w) if float(w) => match c {
            Cond::Lt => fc!(w, vcgtq_f32, vcgtq_f64, y, x),
            Cond::Le => fc!(w, vcgeq_f32, vcgeq_f64, y, x),
            Cond::Gt => fc!(w, vcgtq_f32, vcgtq_f64, x, y),
            Cond::Ge => fc!(w, vcgeq_f32, vcgeq_f64, x, y),
            Cond::Eq => fc!(w, vceqq_f32, vceqq_f64, x, y),
            Cond::Ne => vmvnq_u8(fc!(w, vceqq_f32, vceqq_f64, x, y)),
        },
        VX::ICmp(c, w) | VX::UCmp(c, w) => {
            let s = matches!(op, VX::ICmp(..));
            match c {
                Cond::Lt => ic!(w, y, x, vcgtq_s8, vcgtq_s16, vcgtq_s32, vcgtq_s64, vcgtq_u8, vcgtq_u16, vcgtq_u32, vcgtq_u64, s),
                Cond::Le => ic!(w, y, x, vcgeq_s8, vcgeq_s16, vcgeq_s32, vcgeq_s64, vcgeq_u8, vcgeq_u16, vcgeq_u32, vcgeq_u64, s),
                Cond::Gt => ic!(w, x, y, vcgtq_s8, vcgtq_s16, vcgtq_s32, vcgtq_s64, vcgtq_u8, vcgtq_u16, vcgtq_u32, vcgtq_u64, s),
                Cond::Ge => ic!(w, x, y, vcgeq_s8, vcgeq_s16, vcgeq_s32, vcgeq_s64, vcgeq_u8, vcgeq_u16, vcgeq_u32, vcgeq_u64, s),
                Cond::Eq => ic!(w, x, y, vceqq_s8, vceqq_s16, vceqq_s32, vceqq_s64, vceqq_u8, vceqq_u16, vceqq_u32, vceqq_u64, false),
                Cond::Ne => vmvnq_u8(ic!(w, x, y, vceqq_s8, vceqq_s16, vceqq_s32, vceqq_s64, vceqq_u8, vceqq_u16, vceqq_u32, vceqq_u64, false)),
            }
        }
        VX::ShlV(w) | VX::ArmUShl(w) => sh!(w, x, y, false),
        VX::ArmSShl(w) => sh!(w, x, y, true),
        VX::ShrUV(w) => sh!(w, x, neg(w, y), false),
        VX::ShrSV(w) => sh!(w, x, neg(w, y), true),
        VX::Neg(w) => neg(w, x),
        VX::FNeg(w) if float(w) => f1!(w, vnegq_f32, vnegq_f64),
        VX::FAbs(w) if float(w) => f1!(w, vabsq_f32, vabsq_f64),
        VX::FSqrt(w, n) if float(w) => {
            let r = f1!(w, vsqrtq_f32, vsqrtq_f64);
            if n == NanBits::Intel { intel_nan_v(w, r, x, x) } else { r }
        }
        VX::FRound(w, k) if float(w) => match k {
            RoundTo::Floor => f1!(w, vrndmq_f32, vrndmq_f64),
            RoundTo::Ceil => f1!(w, vrndpq_f32, vrndpq_f64),
            RoundTo::Trunc => f1!(w, vrndq_f32, vrndq_f64),
            RoundTo::Away => f1!(w, vrndaq_f32, vrndaq_f64),
            RoundTo::Even => f1!(w, vrndnq_f32, vrndnq_f64),
        },
        // fcvtzs / scvtf
        VX::FToI(VW::D) => vreinterpretq_u8_s64(vcvtq_s64_f64(vreinterpretq_f64_u8(x))),
        VX::FToI(VW::S) => vreinterpretq_u8_s32(vcvtq_s32_f32(vreinterpretq_f32_u8(x))),
        VX::IToF(VW::D) => vreinterpretq_u8_f64(vcvtq_f64_s64(vreinterpretq_s64_u8(x))),
        VX::IToF(VW::S) => vreinterpretq_u8_f32(vcvtq_f32_s32(vreinterpretq_s32_u8(x))),
        // xtn / xtn2 (w: the source lane width)
        VX::Narrow(VW::H) => vcombine_u8(vmovn_u16(vreinterpretq_u16_u8(x)), vmovn_u16(vreinterpretq_u16_u8(y))),
        VX::Narrow(VW::S) => vreinterpretq_u8_u16(vcombine_u16(vmovn_u32(vreinterpretq_u32_u8(x)), vmovn_u32(vreinterpretq_u32_u8(y)))),
        VX::Narrow(VW::D) => vreinterpretq_u8_u32(vcombine_u32(vmovn_u64(vreinterpretq_u64_u8(x)), vmovn_u64(vreinterpretq_u64_u8(y)))),
        // fcvtn / fcvtn2
        VX::FCvtNarrow => vreinterpretq_u8_f32(vcombine_f32(vcvt_f32_f64(vreinterpretq_f64_u8(x)), vcvt_f32_f64(vreinterpretq_f64_u8(y)))),
        // fcvtl .2d, .2s
        VX::FCvtLo => vreinterpretq_u8_f64(vcvt_f64_f32(vget_low_f32(vreinterpretq_f32_u8(x)))),
        _ => return None,
    })
}

// out of line (inlined, its stack arrays cost the handler its tail jump)
#[inline(never)]
#[target_feature(enable = "neon,sha2,aes")]
unsafe fn vx2(op: VX, a: *const u64, b: *const u64, d: *mut u64) {
    // scalar operands and results first (I cells)
    match op {
        VX::Splat(w) => {
            let x = *a;
            let v = match w {
                VW::B => vdupq_n_u8(x as u8),
                VW::H => vreinterpretq_u8_u16(vdupq_n_u16(x as u16)),
                VW::S => vreinterpretq_u8_u32(vdupq_n_u32(x as u32)),
                VW::D => vreinterpretq_u8_u64(vdupq_n_u64(x)),
            };
            return st(d, v);
        }
        VX::FromLo64 => {
            let x = *a;
            *d = x;
            *d.add(1) = 0;
            return;
        }
        VX::Extract(w, k) => {
            let n = (w.bits() / 8) as usize;
            let mut x = [0u8; 16];
            vst1q_u8(x.as_mut_ptr(), ld(a));
            let mut p = [0u8; 8];
            p[..n].copy_from_slice(&x[k as usize * n..k as usize * n + n]);
            *d = u64::from_le_bytes(p);
            return;
        }
        VX::Insert(w, k) => {
            let n = (w.bits() / 8) as usize;
            let mut x = [0u8; 16];
            vst1q_u8(x.as_mut_ptr(), ld(a));
            x[k as usize * n..k as usize * n + n].copy_from_slice(&(*b).to_le_bytes()[..n]);
            return st(d, vld1q_u8(x.as_ptr()));
        }
        VX::MoveMaskB => {
            let t = vreinterpretq_u64_u8(vshrq_n_u8::<7>(ld(a)));
            let lo = vgetq_lane_u64::<0>(t).wrapping_mul(0x0102_0408_1020_4080) >> 56;
            let hi = vgetq_lane_u64::<1>(t).wrapping_mul(0x0102_0408_1020_4080) >> 56;
            *d = lo | hi << 8;
            return;
        }
        VX::MoveMask(w) => {
            // the top bit of each lane, lane i at bit i
            let mut x = [0u8; 16];
            vst1q_u8(x.as_mut_ptr(), ld(a));
            let n = (w.bits() / 8) as usize;
            let mut r = 0u64;
            for lane in 0..16 / n {
                r |= ((x[lane * n + n - 1] >> 7) as u64) << lane;
            }
            *d = r;
            return;
        }
        VX::AddAcross(w) => {
            let v = ld(a);
            *d = match w {
                VW::B => vaddvq_u8(v) as u64,
                VW::H => vaddvq_u16(vreinterpretq_u16_u8(v)) as u64,
                VW::S => vaddvq_u32(vreinterpretq_u32_u8(v)) as u64,
                VW::D => vaddvq_u64(vreinterpretq_u64_u8(v)),
            };
            return;
        }
        VX::MaxAcrossU(w) => {
            let v = ld(a);
            *d = match w {
                VW::B => vmaxvq_u8(v) as u64,
                VW::H => vmaxvq_u16(vreinterpretq_u16_u8(v)) as u64,
                VW::S => vmaxvq_u32(vreinterpretq_u32_u8(v)) as u64,
                VW::D => {
                    let x = vreinterpretq_u64_u8(v);
                    vgetq_lane_u64::<0>(x).max(vgetq_lane_u64::<1>(x))
                }
            };
            return;
        }
        _ => {}
    }
    let (x, y) = (ld(a), ld(b));
    if let Some(r) = lane_op(op, x, y) {
        return st(d, r);
    }
    let r = match op {
        VX::Add(w) => by_w!(w, x, y, vaddq_u8, vaddq_u16, vaddq_u32, vaddq_u64),
        VX::Sub(w) => by_w!(w, x, y, vsubq_u8, vsubq_u16, vsubq_u32, vsubq_u64),
        VX::MulLo(w) => match w {
            VW::D => lanes(w, x, y, |p, q| p.wrapping_mul(q)),
            _ => by_w!(w, x, y, vmulq_u8, vmulq_u16, vmulq_u32, vaddq_u64),
        },
        VX::MinS(w) => match w {
            VW::D => lanes(w, x, y, |p, q| (p as i64).min(q as i64) as u64),
            _ => by_ws!(w, x, y, vminq_s8, vminq_s16, vminq_s32, vaddq_s64),
        },
        VX::MaxS(w) => match w {
            VW::D => lanes(w, x, y, |p, q| (p as i64).max(q as i64) as u64),
            _ => by_ws!(w, x, y, vmaxq_s8, vmaxq_s16, vmaxq_s32, vaddq_s64),
        },
        VX::MinU(w) => match w {
            VW::D => lanes(w, x, y, |p, q| p.min(q)),
            _ => by_w!(w, x, y, vminq_u8, vminq_u16, vminq_u32, vaddq_u64),
        },
        VX::MaxU(w) => match w {
            VW::D => lanes(w, x, y, |p, q| p.max(q)),
            _ => by_w!(w, x, y, vmaxq_u8, vmaxq_u16, vmaxq_u32, vaddq_u64),
        },
        VX::CmpEq(w) => by_w!(w, x, y, vceqq_u8, vceqq_u16, vceqq_u32, vceqq_u64),
        VX::CmpGtS(w) => by_wc!(w, x, y, vcgtq_s8, vcgtq_s16, vcgtq_s32, vcgtq_s64),
        VX::AddSatU(w) => by_w!(w, x, y, vqaddq_u8, vqaddq_u16, vqaddq_u32, vqaddq_u64),
        VX::SubSatU(w) => by_w!(w, x, y, vqsubq_u8, vqsubq_u16, vqsubq_u32, vqsubq_u64),
        VX::AddSatS(w) => by_ws!(w, x, y, vqaddq_s8, vqaddq_s16, vqaddq_s32, vqaddq_s64),
        VX::SubSatS(w) => by_ws!(w, x, y, vqsubq_s8, vqsubq_s16, vqsubq_s32, vqsubq_s64),
        VX::AvgU(w) => match w {
            VW::D => lanes(w, x, y, |p, q| ((p as u128 + q as u128 + 1) >> 1) as u64),
            _ => by_w!(w, x, y, vrhaddq_u8, vrhaddq_u16, vrhaddq_u32, vaddq_u64),
        },
        // pmull .1q of the selected 64-bit lanes
        VX::Clmul(k) => {
            let (p, q) = (vreinterpretq_u64_u8(x), vreinterpretq_u64_u8(y));
            let p = if k & 1 != 0 { vgetq_lane_u64::<1>(p) } else { vgetq_lane_u64::<0>(p) };
            let q = if k & 2 != 0 { vgetq_lane_u64::<1>(q) } else { vgetq_lane_u64::<0>(q) };
            vreinterpretq_u8_p128(vmull_p64(p, q))
        }
        VX::And => vandq_u8(x, y),
        VX::Or => vorrq_u8(x, y),
        VX::Xor => veorq_u8(x, y),
        VX::AndNot => vbicq_u8(x, y),
        VX::ZipLo(w) => by_w!(w, x, y, vzip1q_u8, vzip1q_u16, vzip1q_u32, vzip1q_u64),
        VX::ZipHi(w) => by_w!(w, x, y, vzip2q_u8, vzip2q_u16, vzip2q_u32, vzip2q_u64),
        VX::MaddH => {
            let (p, q) = (vreinterpretq_s16_u8(x), vreinterpretq_s16_u8(y));
            let lo = vmull_s16(vget_low_s16(p), vget_low_s16(q));
            let hi = vmull_high_s16(p, q);
            vreinterpretq_u8_s32(vpaddq_s32(lo, hi))
        }
        VX::MaddUbs => {
            let a0 = vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(x)));
            let a1 = vreinterpretq_s16_u16(vmovl_high_u8(x));
            let ys = vreinterpretq_s8_u8(y);
            let b0 = vmovl_s8(vget_low_s8(ys));
            let b1 = vmovl_high_s8(ys);
            let m0 = vpaddlq_s16(vmulq_s16(a0, b0));
            let m1 = vpaddlq_s16(vmulq_s16(a1, b1));
            vreinterpretq_u8_s16(vcombine_s16(vqmovn_s32(m0), vqmovn_s32(m1)))
        }
        VX::SadU8 => vreinterpretq_u8_u64(vpaddlq_u32(vpaddlq_u16(vpaddlq_u8(vabdq_u8(x, y))))),
        VX::PackSsH => vreinterpretq_u8_s8(vcombine_s8(vqmovn_s16(vreinterpretq_s16_u8(x)), vqmovn_s16(vreinterpretq_s16_u8(y)))),
        VX::PackUsH => vcombine_u8(vqmovun_s16(vreinterpretq_s16_u8(x)), vqmovun_s16(vreinterpretq_s16_u8(y))),
        VX::PackSsS => vreinterpretq_u8_s16(vcombine_s16(vqmovn_s32(vreinterpretq_s32_u8(x)), vqmovn_s32(vreinterpretq_s32_u8(y)))),
        VX::PackUsS => vreinterpretq_u8_u16(vcombine_u16(vqmovun_s32(vreinterpretq_s32_u8(x)), vqmovun_s32(vreinterpretq_s32_u8(y)))),
        VX::ByteShuffle => vqtbl1q_u8(x, vandq_u8(y, vdupq_n_u8(0x8f))),
        VX::Abs(w) => match w {
            VW::B => vreinterpretq_u8_s8(vabsq_s8(vreinterpretq_s8_u8(x))),
            VW::H => vreinterpretq_u8_s16(vabsq_s16(vreinterpretq_s16_u8(x))),
            VW::S => vreinterpretq_u8_s32(vabsq_s32(vreinterpretq_s32_u8(x))),
            VW::D => vreinterpretq_u8_s64(vabsq_s64(vreinterpretq_s64_u8(x))),
        },
        VX::ShlI(w, k) => {
            let (es, k) = (w.bits(), k as u32);
            lanes(w, x, x, |p, _| if k >= es { 0 } else { p << k })
        }
        VX::ShrUI(w, k) => {
            let (es, k) = (w.bits(), k as u32);
            lanes(w, x, x, |p, _| if k >= es { 0 } else { p >> k })
        }
        VX::ShrSI(w, k) => {
            let es = w.bits();
            let k = (k as u32).min(es - 1);
            lanes(w, x, x, |p, _| (sext(p, es) >> k) as u64)
        }
        VX::ShufS(k) => {
            let mut l = [0u32; 4];
            vst1q_u32(l.as_mut_ptr(), u32x(x));
            let mut r = [0u32; 4];
            for (lane, o) in r.iter_mut().enumerate() {
                *o = l[((k as usize) >> (2 * lane)) & 3];
            }
            u8x(vld1q_u32(r.as_ptr()))
        }
        VX::BytesShrI(k) | VX::BytesShlI(k) => {
            let k = (k as usize).min(16);
            let mut b = [0u8; 16];
            vst1q_u8(b.as_mut_ptr(), x);
            let mut r = [0u8; 16];
            if let VX::BytesShrI(_) = op {
                r[..16 - k].copy_from_slice(&b[k..]);
            } else {
                r[k..].copy_from_slice(&b[..16 - k]);
            }
            vld1q_u8(r.as_ptr())
        }
        VX::WidenLoU(w) | VX::WidenHiU(w) | VX::WidenLoS(w) | VX::WidenHiS(w) => {
            let hi = matches!(op, VX::WidenHiU(_) | VX::WidenHiS(_));
            let signed = matches!(op, VX::WidenLoS(_) | VX::WidenHiS(_));
            match (w, signed, hi) {
                (VW::B, false, false) => vreinterpretq_u8_u16(vmovl_u8(vget_low_u8(x))),
                (VW::B, false, true) => vreinterpretq_u8_u16(vmovl_high_u8(x)),
                (VW::B, true, false) => vreinterpretq_u8_s16(vmovl_s8(vget_low_s8(vreinterpretq_s8_u8(x)))),
                (VW::B, true, true) => vreinterpretq_u8_s16(vmovl_high_s8(vreinterpretq_s8_u8(x))),
                (VW::H, false, false) => vreinterpretq_u8_u32(vmovl_u16(vget_low_u16(vreinterpretq_u16_u8(x)))),
                (VW::H, false, true) => vreinterpretq_u8_u32(vmovl_high_u16(vreinterpretq_u16_u8(x))),
                (VW::H, true, false) => vreinterpretq_u8_s32(vmovl_s16(vget_low_s16(vreinterpretq_s16_u8(x)))),
                (VW::H, true, true) => vreinterpretq_u8_s32(vmovl_high_s16(vreinterpretq_s16_u8(x))),
                (_, false, false) => vreinterpretq_u8_u64(vmovl_u32(vget_low_u32(vreinterpretq_u32_u8(x)))),
                (_, false, true) => vreinterpretq_u8_u64(vmovl_high_u32(vreinterpretq_u32_u8(x))),
                (_, true, false) => vreinterpretq_u8_s64(vmovl_s32(vget_low_s32(vreinterpretq_s32_u8(x)))),
                (_, true, true) => vreinterpretq_u8_s64(vmovl_high_s32(vreinterpretq_s32_u8(x))),
            }
        }
        VX::PairAddLongU(w) => match w {
            VW::B => vreinterpretq_u8_u16(vpaddlq_u8(x)),
            VW::H => vreinterpretq_u8_u32(vpaddlq_u16(vreinterpretq_u16_u8(x))),
            _ => vreinterpretq_u8_u64(vpaddlq_u32(vreinterpretq_u32_u8(x))),
        },
        // ---- SHA: the Arm instructions as the backend uses them
        VX::ArmSha1H => {
            let e = vgetq_lane_u32::<0>(u32x(x));
            u8x(vsetq_lane_u32::<0>(vsha1h_u32(e), vdupq_n_u32(0)))
        }
        VX::ArmSha1Su1 => u8x(vsha1su1q_u32(u32x(x), u32x(y))),
        VX::ArmSha256Su0 => u8x(vsha256su0q_u32(u32x(x), u32x(y))),
        // ---- Intel's SHA semantics through the Arm instructions (arm64.rs `vx`)
        VX::X86Sha1Rnds4(k) => {
            let t = rev4(u32x(x));
            let kw = [0x5a82_7999u32, 0x6ed9_eba1, 0x8f1b_bcdc, 0xca62_c1d6][(k & 3) as usize];
            let wk = vaddq_u32(rev4(u32x(y)), vdupq_n_u32(kw));
            let r = match k {
                0 => vsha1cq_u32(t, 0, wk),
                2 => vsha1mq_u32(t, 0, wk),
                _ => vsha1pq_u32(t, 0, wk),
            };
            u8x(rev4(r))
        }
        VX::X86Sha1NextE => {
            let t = rev4(u32x(x));
            let h = vsetq_lane_u32::<0>(vsha1h_u32(vgetq_lane_u32::<0>(t)), vdupq_n_u32(0));
            // ext #4 of (zero, h): E moves to lane 3
            let e3 = vextq_u32::<1>(vdupq_n_u32(0), h);
            u8x(vaddq_u32(u32x(y), e3))
        }
        VX::X86Sha1Msg1 => u8x(rev4(vsha1su0q_u32(rev4(u32x(x)), rev4(u32x(y)), vdupq_n_u32(0)))),
        VX::X86Sha1Msg2 => u8x(rev4(vsha1su1q_u32(rev4(u32x(x)), rev4(u32x(y))))),
        VX::X86Sha256Msg1 => u8x(vsha256su0q_u32(u32x(x), u32x(y))),
        VX::X86Sha256Msg2 => {
            let t6 = vsetq_lane_u32::<0>(0, u32x(y));
            u8x(vsha256su1q_u32(u32x(x), vdupq_n_u32(0), t6))
        }
        _ => unreachable!("vx {:?} has three operands or a scalar result", op),
    };
    st(d, r)
}

#[inline(never)]
#[target_feature(enable = "neon,sha2")]
unsafe fn vx3(op: VX, a: *const u64, b: *const u64, cc: *const u64, d: *mut u64) {
    let (x, y, z) = (u32x(ld(a)), u32x(ld(b)), u32x(ld(cc)));
    let r = match op {
        VX::ArmSha1C => vsha1cq_u32(x, vgetq_lane_u32::<0>(y), z),
        VX::ArmSha1P => vsha1pq_u32(x, vgetq_lane_u32::<0>(y), z),
        VX::ArmSha1M => vsha1mq_u32(x, vgetq_lane_u32::<0>(y), z),
        VX::ArmSha1Su0 => vsha1su0q_u32(x, y, z),
        VX::ArmSha256H => vsha256hq_u32(x, y, z),
        VX::ArmSha256H2 => vsha256h2q_u32(x, y, z),
        VX::ArmSha256Su1 => vsha256su1q_u32(x, y, z),
        VX::X86Sha256Rnds2 => {
            let (x64, y64) = (vreinterpretq_u64_u32(x), vreinterpretq_u64_u32(y));
            let t4 = rev4(vreinterpretq_u32_u64(vzip2q_u64(x64, y64)));
            let t5 = rev4(vreinterpretq_u32_u64(vzip1q_u64(x64, y64)));
            let t6 = vsha256hq_u32(t4, t5, z);
            let t5b = vsha256h2q_u32(t5, t4, z);
            let t = vreinterpretq_u32_u64(vzip2q_u64(vreinterpretq_u64_u32(t5b), vreinterpretq_u64_u32(t6)));
            vrev64q_u32(t)
        }
        _ => unreachable!("vx3 {:?}", op),
    };
    st(d, u8x(r))
}

/// [h, d|a|b, op] (op: address of the VX value)
unsafe extern "C" fn h_vx(pc: *const u64, fp: *mut u64, cx: *mut Cx) {
    let w = *pc.add(1);
    let op = *(*pc.add(2) as *const VX);
    vx2(op, c(fp, w, 1), c(fp, w, 2), c(fp, w, 0));
    next!(pc.add(3), fp, cx)
}

/// [h, d|a|b|c, op]
unsafe extern "C" fn h_vx3(pc: *const u64, fp: *mut u64, cx: *mut Cx) {
    let w = *pc.add(1);
    let op = *(*pc.add(2) as *const VX);
    vx3(op, c(fp, w, 1), c(fp, w, 2), c(fp, w, 3), c(fp, w, 0));
    next!(pc.add(3), fp, cx)
}

pub(super) fn handlers() -> (H, H) {
    (h_vx, h_vx3)
}
