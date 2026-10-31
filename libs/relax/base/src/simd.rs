//! core::arch intrinsics as vector instructions. std implements every intrinsic in scalar
//! Rust (the fallback and the reference); when a slot of a known intrinsic is built, its
//! lowered body is replaced here by lane-typed vector RIR (`Inst::VX`): vector arguments
//! are loaded from the addresses lowering passes them by (aggregates by pointer), the result
//! is stored through the result address (or returned, for scalars). After inlining, opt's
//! vector store->load forwarding keeps chains of intrinsics in vector registers.
//!
//! 64-bit NEON vectors (uint8x8_t ...) live in the low half of a V128; 256-bit AVX vectors
//! are two V128 halves (every AVX2 intrinsic here works per 128-bit lane or per half).

use relax_backend::rir::{Block, Cls, Func, Inst, IntTy, Mem, Term, VReg, VW, VX};

#[derive(Clone, Copy, PartialEq)]
enum K {
    /// 128-bit vector (by pointer)
    V,
    /// 64-bit vector (by pointer, 8 bytes)
    N,
    /// 256-bit vector (by pointer, two halves)
    W,
    /// scalar integer (value)
    I,
}

#[derive(Clone, Copy)]
enum Val {
    V(VReg),
    W(VReg, VReg),
    S(VReg),
}

struct Bd<'a> {
    f: &'a mut Func,
    insts: Vec<Inst>,
}

impl<'a> Bd<'a> {
    fn vreg(&mut self, c: Cls) -> VReg {
        self.f.vreg(c)
    }
    fn vx(&mut self, op: VX, a: VReg, b: VReg) -> VReg {
        let d = self.vreg(if op.scalar_result() { Cls::I } else { Cls::V128 });
        self.insts.push(Inst::VX(op, d, a, b));
        d
    }
    fn vx1(&mut self, op: VX, a: VReg) -> VReg {
        self.vx(op, a, a)
    }
    fn iconst(&mut self, k: i64) -> VReg {
        let d = self.vreg(Cls::I);
        self.insts.push(Inst::Iconst(d, k));
        d
    }
    fn zero(&mut self) -> VReg {
        let z = self.iconst(0);
        self.vx1(VX::FromLo64, z)
    }
    /// a & !b
    fn andnot(&mut self, a: VReg, b: VReg) -> VReg {
        self.vx(VX::AndNot, a, b)
    }
    fn sext32(&mut self, v: VReg) -> VReg {
        let d = self.vreg(Cls::I);
        self.insts.push(Inst::Conv(relax_backend::rir::Conv::IntToInt(IntTy { bits: 32, signed: true }), d, v));
        d
    }
    /// build a vector from 64-bit halves (scalars)
    fn from_halves(&mut self, lo: VReg, hi: VReg) -> VReg {
        let v = self.vx1(VX::FromLo64, lo);
        self.vx(VX::Insert(VW::D, 1), v, hi)
    }
}

fn v(x: Val) -> VReg {
    match x {
        Val::V(r) | Val::S(r) => r,
        Val::W(lo, _) => lo,
    }
}

fn w(x: Val) -> (VReg, VReg) {
    match x {
        Val::W(lo, hi) => (lo, hi),
        Val::V(r) | Val::S(r) => (r, r),
    }
}

type Gen = fn(&mut Bd, &[Val], &[i64]) -> Option<Val>;

/// The intrinsic table: argument kinds, result kind, body. `None` from a body: this
/// instantiation (constant) is not covered, keep the scalar version.
fn table(name: &str) -> Option<(&'static [K], Option<K>, Gen)> {
    use K::*;
    use VW::*;
    macro_rules! bin {
        ($op:expr) => {
            |b: &mut Bd, x: &[Val], _: &[i64]| Some(Val::V(b.vx($op, v(x[0]), v(x[1]))))
        };
    }
    macro_rules! bin256 {
        ($op:expr) => {
            |b: &mut Bd, x: &[Val], _: &[i64]| {
                let (al, ah) = w(x[0]);
                let (bl, bh) = w(x[1]);
                let lo = b.vx($op, al, bl);
                let hi = b.vx($op, ah, bh);
                Some(Val::W(lo, hi))
            }
        };
    }
    let r: (&'static [K], Option<K>, Gen) = match name {
        // ---------------------------------------------------------------- aarch64 (NEON)
        "vaddq_s16" => (&[V, V], Some(V), bin!(VX::Add(H))),
        "vaddq_s32" | "vaddq_u32" => (&[V, V], Some(V), bin!(VX::Add(S))),
        "vsubq_s16" => (&[V, V], Some(V), bin!(VX::Sub(H))),
        "vsubq_s32" => (&[V, V], Some(V), bin!(VX::Sub(S))),
        "vmulq_s16" => (&[V, V], Some(V), bin!(VX::MulLo(H))),
        "vmulq_s32" => (&[V, V], Some(V), bin!(VX::MulLo(S))),
        "vmaxq_s16" => (&[V, V], Some(V), bin!(VX::MaxS(H))),
        "vminq_s16" => (&[V, V], Some(V), bin!(VX::MinS(H))),
        "vorrq_s32" => (&[V, V], Some(V), bin!(VX::Or)),
        "vzip1q_s16" => (&[V, V], Some(V), bin!(VX::ZipLo(H))),
        "vzip2q_s16" => (&[V, V], Some(V), bin!(VX::ZipHi(H))),
        "vtrn1q_s64" => (&[V, V], Some(V), bin!(VX::ZipLo(D))),
        "vtrn2q_s64" => (&[V, V], Some(V), bin!(VX::ZipHi(D))),
        "vdupq_n_u8" => (&[I], Some(V), |b, x, _| Some(Val::V(b.vx1(VX::Splat(B), v(x[0]))))),
        "vdupq_n_s16" | "vdupq_n_u16" => (&[I], Some(V), |b, x, _| Some(Val::V(b.vx1(VX::Splat(H), v(x[0]))))),
        "vdupq_n_s32" | "vdupq_n_u32" => (&[I], Some(V), |b, x, _| Some(Val::V(b.vx1(VX::Splat(S), v(x[0]))))),
        "vgetq_lane_u32" => (&[V], Some(I), |b, x, c| {
            let k = *c.first()?;
            (0..4).contains(&k).then(|| Val::S(b.vx1(VX::Extract(S, k as u8), v(x[0]))))
        }),
        "vget_lane_u32" => (&[N], Some(I), |b, x, c| {
            let k = *c.first()?;
            (0..2).contains(&k).then(|| Val::S(b.vx1(VX::Extract(S, k as u8), v(x[0]))))
        }),
        "vsetq_lane_u32" => (&[I, V], Some(V), |b, x, c| {
            let k = *c.first()?;
            (0..4).contains(&k).then(|| Val::V(b.vx(VX::Insert(S, k as u8), v(x[1]), v(x[0]))))
        }),
        "vaddvq_u32" => (&[V], Some(I), |b, x, _| Some(Val::S(b.vx1(VX::AddAcross(S), v(x[0]))))),
        "vmaxvq_u32" => (&[V], Some(I), |b, x, _| Some(Val::S(b.vx1(VX::MaxAcrossU(S), v(x[0]))))),
        "vaddw_u8" => (&[V, N], Some(V), |b, x, _| {
            let wide = b.vx1(VX::WidenLoU(B), v(x[1]));
            Some(Val::V(b.vx(VX::Add(H), v(x[0]), wide)))
        }),
        "vmlal_u16" => (&[V, N, N], Some(V), |b, x, _| {
            let p = b.vx1(VX::WidenLoU(H), v(x[1]));
            let q = b.vx1(VX::WidenLoU(H), v(x[2]));
            let m = b.vx(VX::MulLo(S), p, q);
            Some(Val::V(b.vx(VX::Add(S), v(x[0]), m)))
        }),
        "vmlal_high_u16" => (&[V, V, V], Some(V), |b, x, _| {
            let p = b.vx1(VX::WidenHiU(H), v(x[1]));
            let q = b.vx1(VX::WidenHiU(H), v(x[2]));
            let m = b.vx(VX::MulLo(S), p, q);
            Some(Val::V(b.vx(VX::Add(S), v(x[0]), m)))
        }),
        "vpadalq_u8" => (&[V, V], Some(V), |b, x, _| {
            let p = b.vx1(VX::PairAddLongU(B), v(x[1]));
            Some(Val::V(b.vx(VX::Add(H), v(x[0]), p)))
        }),
        "vpadalq_u16" => (&[V, V], Some(V), |b, x, _| {
            let p = b.vx1(VX::PairAddLongU(H), v(x[1]));
            Some(Val::V(b.vx(VX::Add(S), v(x[0]), p)))
        }),
        "vpaddlq_u8" => (&[V], Some(V), |b, x, _| Some(Val::V(b.vx1(VX::PairAddLongU(B), v(x[0]))))),
        "vshlq_n_u32" => (&[V], Some(V), |b, x, c| {
            let k = *c.first()?;
            (0..32).contains(&k).then(|| Val::V(b.vx1(VX::ShlI(S, k as u8), v(x[0]))))
        }),
        // saturating: lanes with bits shifted out become all ones
        "vqshlq_n_u32" => (&[V], Some(V), |b, x, c| {
            let k = *c.first()?;
            if !(0..32).contains(&k) {
                return None;
            }
            if k == 0 {
                return Some(Val::V(v(x[0])));
            }
            let a = v(x[0]);
            let sh = b.vx1(VX::ShlI(S, k as u8), a);
            let out = b.vx1(VX::ShrUI(S, (32 - k) as u8), a);
            let z = b.zero();
            let keep = b.vx(VX::CmpEq(S), out, z);
            let ones = b.vx(VX::CmpEq(S), z, z);
            let sat = b.andnot(ones, keep);
            Some(Val::V(b.vx(VX::Or, sh, sat)))
        }),
        "vshrq_n_s16" => (&[V], Some(V), |b, x, c| {
            let k = *c.first()?;
            (1..=16).contains(&k).then(|| Val::V(b.vx1(VX::ShrSI(H, k as u8), v(x[0]))))
        }),
        "vshrq_n_s32" => (&[V], Some(V), |b, x, c| {
            let k = *c.first()?;
            (1..=32).contains(&k).then(|| Val::V(b.vx1(VX::ShrSI(S, k as u8), v(x[0]))))
        }),
        "vget_low_u8" | "vget_low_u16" | "vget_low_u32" | "vget_low_s16" => (&[V], Some(N), |_, x, _| Some(Val::V(v(x[0])))),
        "vget_high_u8" | "vget_high_u16" | "vget_high_u32" => (&[V], Some(N), |b, x, _| Some(Val::V(b.vx1(VX::BytesShrI(8), v(x[0]))))),
        "vcombine_u8" | "vcombine_u16" | "vcombine_s16" => (&[N, N], Some(V), bin!(VX::ZipLo(D))),
        "vld1q_u8" | "vld1q_u16" | "vld1q_u32" | "vld1q_s16" => (&[I], Some(V), |b, x, _| {
            let d = b.vreg(Cls::V128);
            b.insts.push(Inst::VLoad(d, v(x[0]), 0));
            Some(Val::V(d))
        }),
        "vceq_u8" => (&[N, N], Some(N), bin!(VX::CmpEq(B))),
        // a < 0 per signed byte: 0 > a
        "vcltz_s8" => (&[N], Some(N), |b, x, _| {
            let z = b.zero();
            Some(Val::V(b.vx(VX::CmpGtS(B), z, v(x[0]))))
        }),
        "vdup_n_u8" => (&[I], Some(N), |b, x, _| Some(Val::V(b.vx1(VX::Splat(B), v(x[0]))))),
        "vreinterpret_u64_u8" => (&[N], Some(N), |_, x, _| Some(Val::V(v(x[0])))),
        "vget_lane_u64" => (&[N], Some(I), |b, x, c| {
            let k = *c.first()?;
            (k == 0).then(|| Val::S(b.vx1(VX::Extract(D, 0), v(x[0]))))
        }),
        "vld1_u8" => (&[I], Some(N), |b, x, _| {
            let t = b.vreg(Cls::I);
            b.insts.push(Inst::Load(Mem::Int(8, false), t, v(x[0]), 0));
            Some(Val::V(b.vx1(VX::FromLo64, t)))
        }),
        "vld1_u16" => (&[I], Some(N), |b, x, _| {
            let t = b.vreg(Cls::I);
            b.insts.push(Inst::Load(Mem::Int(8, false), t, v(x[0]), 0));
            Some(Val::V(b.vx1(VX::FromLo64, t)))
        }),
        "vst1q_u32" | "vst1q_s16" => (&[I, V], None, |b, x, _| {
            b.insts.push(Inst::VStore(v(x[0]), 0, v(x[1])));
            Some(Val::S(v(x[0])))
        }),
        // ---------------------------------------------------------------- x86_64 (SSE2..4.1)
        "_mm_add_epi8" => (&[V, V], Some(V), bin!(VX::Add(B))),
        "_mm_add_epi16" => (&[V, V], Some(V), bin!(VX::Add(H))),
        "_mm_add_epi32" => (&[V, V], Some(V), bin!(VX::Add(S))),
        "_mm_sub_epi8" => (&[V, V], Some(V), bin!(VX::Sub(B))),
        "_mm_sub_epi16" => (&[V, V], Some(V), bin!(VX::Sub(H))),
        "_mm_and_si128" => (&[V, V], Some(V), bin!(VX::And)),
        "_mm_or_si128" => (&[V, V], Some(V), bin!(VX::Or)),
        "_mm_xor_si128" => (&[V, V], Some(V), bin!(VX::Xor)),
        "_mm_andnot_si128" => (&[V, V], Some(V), |b, x, _| Some(Val::V(b.andnot(v(x[1]), v(x[0]))))),
        "_mm_avg_epu8" => (&[V, V], Some(V), bin!(VX::AvgU(B))),
        "_mm_cmpeq_epi16" => (&[V, V], Some(V), bin!(VX::CmpEq(H))),
        "_mm_cmpeq_epi8" => (&[V, V], Some(V), bin!(VX::CmpEq(B))),
        "_mm_cmpgt_epi8" => (&[V, V], Some(V), bin!(VX::CmpGtS(B))),
        // a 16-bit mask: non-negative, the i32 needs no sign extension
        "_mm_movemask_epi8" => (&[V], Some(I), |b, x, _| Some(Val::S(b.vx1(VX::MoveMaskB, v(x[0]))))),
        "_mm_min_epi16" => (&[V, V], Some(V), bin!(VX::MinS(H))),
        "_mm_madd_epi16" => (&[V, V], Some(V), bin!(VX::MaddH)),
        "_mm_maddubs_epi16" => (&[V, V], Some(V), bin!(VX::MaddUbs)),
        "_mm_sad_epu8" => (&[V, V], Some(V), bin!(VX::SadU8)),
        "_mm_packus_epi16" => (&[V, V], Some(V), bin!(VX::PackUsH)),
        "_mm_unpacklo_epi8" => (&[V, V], Some(V), bin!(VX::ZipLo(B))),
        "_mm_unpackhi_epi8" => (&[V, V], Some(V), bin!(VX::ZipHi(B))),
        "_mm_unpackhi_epi64" => (&[V, V], Some(V), bin!(VX::ZipHi(D))),
        "_mm_shuffle_epi8" => (&[V, V], Some(V), bin!(VX::ByteShuffle)),
        "_mm_abs_epi16" => (&[V], Some(V), |b, x, _| Some(Val::V(b.vx1(VX::Abs(H), v(x[0]))))),
        "_mm_shuffle_epi32" => (&[V], Some(V), |b, x, c| {
            let k = *c.first()?;
            (0..256).contains(&k).then(|| Val::V(b.vx1(VX::ShufS(k as u8), v(x[0]))))
        }),
        "_mm_slli_epi32" => (&[V], Some(V), |b, x, c| {
            let k = *c.first()?;
            Some(if (0..32).contains(&k) { Val::V(b.vx1(VX::ShlI(S, k as u8), v(x[0]))) } else { Val::V(b.zero()) })
        }),
        "_mm_srli_si128" => (&[V], Some(V), |b, x, c| {
            let k = (*c.first()? & 0xff).min(16);
            Some(Val::V(b.vx1(VX::BytesShrI(k as u8), v(x[0]))))
        }),
        "_mm_set1_epi8" => (&[I], Some(V), |b, x, _| Some(Val::V(b.vx1(VX::Splat(B), v(x[0]))))),
        "_mm_set1_epi16" => (&[I], Some(V), |b, x, _| Some(Val::V(b.vx1(VX::Splat(H), v(x[0]))))),
        "_mm_setzero_si128" => (&[], Some(V), |b, _, _| Some(Val::V(b.zero()))),
        "_mm_set_epi64x" => (&[I, I], Some(V), |b, x, _| Some(Val::V(b.from_halves(v(x[1]), v(x[0]))))),
        "_mm_cvtsi32_si128" => (&[I], Some(V), |b, x, _| {
            let m = b.iconst(0xffff_ffff);
            let t = b.vreg(Cls::I);
            b.insts.push(Inst::IBin(relax_backend::rir::IOp::And, IntTy { bits: 64, signed: false }, t, v(x[0]), m));
            Some(Val::V(b.vx1(VX::FromLo64, t)))
        }),
        "_mm_cvtsi128_si32" => (&[V], Some(I), |b, x, _| {
            let e = b.vx1(VX::Extract(S, 0), v(x[0]));
            Some(Val::S(b.sext32(e)))
        }),
        "_mm_extract_epi32" => (&[V], Some(I), |b, x, c| {
            let k = *c.first()? & 3;
            let e = b.vx1(VX::Extract(S, k as u8), v(x[0]));
            Some(Val::S(b.sext32(e)))
        }),
        "_mm_loadu_si128" | "_mm_load_si128" => (&[I], Some(V), |b, x, _| {
            let d = b.vreg(Cls::V128);
            b.insts.push(Inst::VLoad(d, v(x[0]), 0));
            Some(Val::V(d))
        }),
        "_mm_storeu_si128" | "_mm_store_si128" => (&[I, V], None, |b, x, _| {
            b.insts.push(Inst::VStore(v(x[0]), 0, v(x[1])));
            Some(Val::S(v(x[0])))
        }),
        // ---- AVX2, per 128-bit half
        "_mm256_add_epi16" => (&[W, W], Some(W), bin256!(VX::Add(H))),
        "_mm256_add_epi32" => (&[W, W], Some(W), bin256!(VX::Add(S))),
        "_mm256_sub_epi16" => (&[W, W], Some(W), bin256!(VX::Sub(H))),
        "_mm256_sub_epi32" => (&[W, W], Some(W), bin256!(VX::Sub(S))),
        "_mm256_mullo_epi16" => (&[W, W], Some(W), bin256!(VX::MulLo(H))),
        "_mm256_mullo_epi32" => (&[W, W], Some(W), bin256!(VX::MulLo(S))),
        "_mm256_max_epi16" => (&[W, W], Some(W), bin256!(VX::MaxS(H))),
        "_mm256_min_epi16" => (&[W, W], Some(W), bin256!(VX::MinS(H))),
        "_mm256_or_si256" => (&[W, W], Some(W), bin256!(VX::Or)),
        "_mm256_madd_epi16" => (&[W, W], Some(W), bin256!(VX::MaddH)),
        "_mm256_maddubs_epi16" => (&[W, W], Some(W), bin256!(VX::MaddUbs)),
        "_mm256_sad_epu8" => (&[W, W], Some(W), bin256!(VX::SadU8)),
        "_mm256_packus_epi16" => (&[W, W], Some(W), bin256!(VX::PackUsH)),
        "_mm256_packus_epi32" => (&[W, W], Some(W), bin256!(VX::PackUsS)),
        "_mm256_packs_epi32" => (&[W, W], Some(W), bin256!(VX::PackSsS)),
        "_mm256_shuffle_epi8" => (&[W, W], Some(W), bin256!(VX::ByteShuffle)),
        "_mm256_unpacklo_epi8" => (&[W, W], Some(W), bin256!(VX::ZipLo(B))),
        "_mm256_unpackhi_epi8" => (&[W, W], Some(W), bin256!(VX::ZipHi(B))),
        "_mm256_unpacklo_epi16" => (&[W, W], Some(W), bin256!(VX::ZipLo(H))),
        "_mm256_unpackhi_epi16" => (&[W, W], Some(W), bin256!(VX::ZipHi(H))),
        "_mm256_unpacklo_epi32" => (&[W, W], Some(W), bin256!(VX::ZipLo(S))),
        "_mm256_unpackhi_epi32" => (&[W, W], Some(W), bin256!(VX::ZipHi(S))),
        "_mm256_unpacklo_epi64" => (&[W, W], Some(W), bin256!(VX::ZipLo(D))),
        "_mm256_unpackhi_epi64" => (&[W, W], Some(W), bin256!(VX::ZipHi(D))),
        "_mm256_set1_epi16" => (&[I], Some(W), |b, x, _| {
            let s = b.vx1(VX::Splat(H), v(x[0]));
            Some(Val::W(s, s))
        }),
        "_mm256_set1_epi32" => (&[I], Some(W), |b, x, _| {
            let s = b.vx1(VX::Splat(S), v(x[0]));
            Some(Val::W(s, s))
        }),
        "_mm256_setzero_si256" => (&[], Some(W), |b, _, _| {
            let z = b.zero();
            Some(Val::W(z, z))
        }),
        "_mm256_slli_epi16" | "_mm256_slli_epi32" | "_mm256_srai_epi16" | "_mm256_srai_epi32" => (&[W], Some(W), |_, _, _| None),
        "_mm256_castsi256_si128" => (&[W], Some(V), |_, x, _| Some(Val::V(w(x[0]).0))),
        "_mm256_extracti128_si256" | "_mm256_extractf128_si256" => (&[W], Some(V), |_, x, c| {
            let k = *c.first()? & 1;
            let (lo, hi) = w(x[0]);
            Some(Val::V(if k == 0 { lo } else { hi }))
        }),
        "_mm256_loadu_si256" => (&[I], Some(W), |b, x, _| {
            let lo = b.vreg(Cls::V128);
            let hi = b.vreg(Cls::V128);
            b.insts.push(Inst::VLoad(lo, v(x[0]), 0));
            b.insts.push(Inst::VLoad(hi, v(x[0]), 16));
            Some(Val::W(lo, hi))
        }),
        "_mm256_storeu_si256" => (&[I, W], None, |b, x, _| {
            let (lo, hi) = w(x[1]);
            b.insts.push(Inst::VStore(v(x[0]), 0, lo));
            b.insts.push(Inst::VStore(v(x[0]), 16, hi));
            Some(Val::S(v(x[0])))
        }),
        _ => return None,
    };
    Some(r)
}

/// Shift by an immediate for the 256-bit forms the table defers (per half)
fn shift256(name: &str, b: &mut Bd, x: &[Val], c: &[i64]) -> Option<Val> {
    let k = *c.first()?;
    let (lo, hi) = w(x[0]);
    let (op, width) = match name {
        "_mm256_slli_epi16" => (0, 16),
        "_mm256_slli_epi32" => (0, 32),
        "_mm256_srai_epi16" => (1, 16),
        "_mm256_srai_epi32" => (1, 32),
        _ => return None,
    };
    let vw = if width == 16 { VW::H } else { VW::S };
    if op == 0 {
        if !(0..width).contains(&k) {
            let z = b.zero();
            return Some(Val::W(z, z));
        }
        let l = b.vx1(VX::ShlI(vw, k as u8), lo);
        let h = b.vx1(VX::ShlI(vw, k as u8), hi);
        Some(Val::W(l, h))
    } else {
        let k = k.clamp(0, width) as u8;
        if k == 0 {
            return Some(Val::W(lo, hi));
        }
        let l = b.vx1(VX::ShrSI(vw, k), lo);
        let h = b.vx1(VX::ShrSI(vw, k), hi);
        Some(Val::W(l, h))
    }
}

/// MAKEPAD_RUN_NO_SIMD=1: std's scalar intrinsics
fn off(options: &crate::options::Options) -> bool {
    options.no_simd
}

/// A core::arch intrinsic this module lowers: its body is a few vector instructions, inlined
/// always (std's scalar body, which the inliner would size, is not what runs).
pub fn covered(options: &crate::options::Options, name: &str) -> bool {
    match name.strip_prefix("core::arch::aarch64::").or_else(|| name.strip_prefix("core::arch::x86_64::")) {
        Some(s) => !off(options) && table(s).is_some(),
        None => false,
    }
}

/// Replaces a known intrinsic's lowered body with vector RIR. `consts`: its const generic
/// arguments. Returns the function unchanged when the intrinsic (or this instance) is not
/// covered or its lowered signature is not the expected one.
pub fn lower_intrinsic(options: &crate::options::Options, name: &str, consts: &[i64], mut f: Func) -> Func {
    if off(options) || f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::VX(..) | Inst::VLoad(..) | Inst::VStore(..)))) {
        // Explicit relax-intrinsic bridges already supplied vector RIR. Retain the
        // legacy table only for std wrappers that still have scalar bodies.
        return f;
    }
    let short = match name.strip_prefix("core::arch::aarch64::").or_else(|| name.strip_prefix("core::arch::x86_64::")) {
        Some(s) => s,
        None => return f,
    };
    let (args, ret, gen) = match table(short) {
        Some(t) => t,
        None => return f,
    };
    // lowered shape: [result address if a vector result] + one param per argument
    let vec_ret = matches!(ret, Some(K::V) | Some(K::N) | Some(K::W));
    let np = args.len() + vec_ret as usize;
    if f.params.len() != np || f.cabi.is_some() {
        return f;
    }
    if vec_ret && !f.rets.is_empty() || matches!(ret, Some(K::I)) && f.rets.len() != 1 || ret.is_none() && !f.rets.is_empty() {
        return f;
    }
    for (k, a) in args.iter().enumerate() {
        if f.vregs[f.params[k + vec_ret as usize].0 as usize] != Cls::I {
            return f;
        }
        let _ = a;
    }
    let params = f.params.clone();
    let pos = f.blocks.first().and_then(|b| b.pos.first().copied()).unwrap_or(0);
    let mut b = Bd { f: &mut f, insts: Vec::new() };
    let mut vals = Vec::new();
    for (k, a) in args.iter().enumerate() {
        let p = params[k + vec_ret as usize];
        vals.push(match a {
            K::V => {
                let d = b.vreg(Cls::V128);
                b.insts.push(Inst::VLoad(d, p, 0));
                Val::V(d)
            }
            K::N => {
                let t = b.vreg(Cls::I);
                b.insts.push(Inst::Load(Mem::Int(8, false), t, p, 0));
                Val::V(b.vx1(VX::FromLo64, t))
            }
            K::W => {
                let lo = b.vreg(Cls::V128);
                let hi = b.vreg(Cls::V128);
                b.insts.push(Inst::VLoad(lo, p, 0));
                b.insts.push(Inst::VLoad(hi, p, 16));
                Val::W(lo, hi)
            }
            K::I => Val::S(p),
        });
    }
    let r = match gen(&mut b, &vals, consts).or_else(|| shift256(short, &mut b, &vals, consts)) {
        Some(r) => r,
        None => {
            drop(b);
            return f;
        }
    };
    let mut rets = Vec::new();
    match ret {
        Some(K::V) => b.insts.push(Inst::VStore(params[0], 0, v(r))),
        Some(K::N) => {
            let t = b.vx1(VX::Extract(VW::D, 0), v(r));
            b.insts.push(Inst::Store(Mem::Int(8, false), params[0], 0, t));
        }
        Some(K::W) => {
            let (lo, hi) = w(r);
            b.insts.push(Inst::VStore(params[0], 0, lo));
            b.insts.push(Inst::VStore(params[0], 16, hi));
        }
        Some(K::I) => rets.push(v(r)),
        None => {}
    }
    let insts = std::mem::take(&mut b.insts);
    drop(b);
    let n = insts.len();
    f.blocks = vec![Block { insts, pos: vec![pos; n], term: Term::Ret(rets), term_pos: pos, count: -1.0 }];
    f
}
