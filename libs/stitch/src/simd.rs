//! 128-bit packed values (the Wasm `v128` value type) and their lane operations.
//!
//! This backs the subset of the Wasm SIMD proposal that stitch implements
//! (see `decode_simd_instr` in code.rs), plus the nonstandard packed float
//! math opcodes (see `decode_ext_math_instr`).
//!
//! NOTE on SIMD codegen: this crate builds on stable Rust, where
//! `std::simd` (portable SIMD) is not available. The lane operations are
//! therefore written as whole-array `[f32; 4]` operations behind this
//! newtype, which LLVM auto-vectorizes on aarch64/x86-64 in release builds.
//! The API is shaped like portable SIMD so the arithmetic core can be
//! swapped to `core::simd::f32x4` if the toolchain ever moves to nightly.

use crate::ops::FloatOps;

/// A 128-bit packed value (Wasm `v128`).
///
/// Stored as raw bits; lane views are provided by the `to_*`/`from_*`
/// methods. Always 16-byte aligned so it can be read from a stack slot
/// with an aligned load.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(C, align(16))]
pub struct V128(u128);

impl V128 {
    pub const ZERO: Self = Self(0);

    pub const fn from_bits(bits: u128) -> Self {
        Self(bits)
    }

    pub const fn to_bits(self) -> u128 {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(u128::from_le_bytes(bytes))
    }

    pub fn to_bytes(self) -> [u8; 16] {
        self.0.to_le_bytes()
    }

    pub fn from_f32x4(lanes: [f32; 4]) -> Self {
        let mut bytes = [0u8; 16];
        for (i, lane) in lanes.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&lane.to_le_bytes());
        }
        Self::from_bytes(bytes)
    }

    pub fn to_f32x4(self) -> [f32; 4] {
        let bytes = self.to_bytes();
        let mut lanes = [0f32; 4];
        for (i, lane) in lanes.iter_mut().enumerate() {
            *lane = f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        }
        lanes
    }

    pub fn from_u32x4(lanes: [u32; 4]) -> Self {
        let mut bytes = [0u8; 16];
        for (i, lane) in lanes.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&lane.to_le_bytes());
        }
        Self::from_bytes(bytes)
    }

    pub fn to_u32x4(self) -> [u32; 4] {
        let bytes = self.to_bytes();
        let mut lanes = [0u32; 4];
        for (i, lane) in lanes.iter_mut().enumerate() {
            *lane = u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        }
        lanes
    }

    /// `f32x4.splat`
    pub fn f32x4_splat(x: f32) -> Self {
        Self::from_f32x4([x; 4])
    }

    /// `f32x4.extract_lane l`
    pub fn f32x4_extract_lane(self, lane: usize) -> f32 {
        self.to_f32x4()[lane]
    }

    /// `f32x4.replace_lane l`
    pub fn f32x4_replace_lane(self, lane: usize, x: f32) -> Self {
        let mut lanes = self.to_f32x4();
        lanes[lane] = x;
        Self::from_f32x4(lanes)
    }

    /// `i8x16.shuffle`: byte `i` of the result is byte `lanes[i]` of the
    /// 32-byte concatenation of `self` and `other`. Lane indices must be
    /// < 32 (validated at decode time).
    pub fn i8x16_shuffle(self, other: Self, lanes: [u8; 16]) -> Self {
        let a = self.to_bytes();
        let b = other.to_bytes();
        let mut out = [0u8; 16];
        for (i, lane) in lanes.iter().enumerate() {
            let lane = *lane as usize;
            out[i] = if lane < 16 { a[lane] } else { b[lane - 16] };
        }
        Self::from_bytes(out)
    }

    // Bitwise operations (`v128.not`/`and`/`andnot`/`or`/`xor`/`bitselect`/`any_true`).

    pub fn not(self) -> Self {
        Self(!self.0)
    }

    pub fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// `v128.andnot`: `a & !b`.
    pub fn andnot(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn xor(self, other: Self) -> Self {
        Self(self.0 ^ other.0)
    }

    /// `v128.bitselect(v1, v2, c)`: bits of `v1` where `c` is 1, bits of
    /// `v2` where `c` is 0.
    pub fn bitselect(self, other: Self, control: Self) -> Self {
        Self((self.0 & control.0) | (other.0 & !control.0))
    }

    /// `v128.any_true`
    pub fn any_true(self) -> u32 {
        (self.0 != 0) as u32
    }

    // f32x4 lane arithmetic (Wasm SIMD spec semantics; the rounding of every
    // lane is the IEEE 754 operation, identical to the scalar f32 opcodes).

    fn map(self, f: impl Fn(f32) -> f32) -> Self {
        let a = self.to_f32x4();
        Self::from_f32x4([f(a[0]), f(a[1]), f(a[2]), f(a[3])])
    }

    fn zip(self, other: Self, f: impl Fn(f32, f32) -> f32) -> Self {
        let a = self.to_f32x4();
        let b = other.to_f32x4();
        Self::from_f32x4([
            f(a[0], b[0]),
            f(a[1], b[1]),
            f(a[2], b[2]),
            f(a[3], b[3]),
        ])
    }

    fn cmp(self, other: Self, f: impl Fn(f32, f32) -> bool) -> Self {
        let a = self.to_f32x4();
        let b = other.to_f32x4();
        let lane = |i: usize| if f(a[i], b[i]) { u32::MAX } else { 0 };
        Self::from_u32x4([lane(0), lane(1), lane(2), lane(3)])
    }

    /// `f32x4.abs` (sign-bit operation, exact even on NaN).
    pub fn f32x4_abs(self) -> Self {
        Self(self.0 & !0x8000_0000_8000_0000_8000_0000_8000_0000u128)
    }

    /// `f32x4.neg` (sign-bit operation, exact even on NaN).
    pub fn f32x4_neg(self) -> Self {
        Self(self.0 ^ 0x8000_0000_8000_0000_8000_0000_8000_0000u128)
    }

    pub fn f32x4_sqrt(self) -> Self {
        self.map(|x| x.sqrt())
    }

    pub fn f32x4_ceil(self) -> Self {
        self.map(|x| float_op(x, FloatOps::ceil))
    }

    pub fn f32x4_floor(self) -> Self {
        self.map(|x| float_op(x, FloatOps::floor))
    }

    pub fn f32x4_trunc(self) -> Self {
        self.map(|x| float_op(x, FloatOps::trunc))
    }

    pub fn f32x4_nearest(self) -> Self {
        self.map(|x| float_op(x, FloatOps::nearest))
    }

    pub fn f32x4_add(self, other: Self) -> Self {
        self.zip(other, |a, b| a + b)
    }

    pub fn f32x4_sub(self, other: Self) -> Self {
        self.zip(other, |a, b| a - b)
    }

    pub fn f32x4_mul(self, other: Self) -> Self {
        self.zip(other, |a, b| a * b)
    }

    pub fn f32x4_div(self, other: Self) -> Self {
        self.zip(other, |a, b| a / b)
    }

    /// `f32x4.min` (Wasm semantics: NaN-propagating, -0 < +0). Uses the
    /// same lane function as the scalar `f32.min` opcode.
    pub fn f32x4_min(self, other: Self) -> Self {
        self.zip(other, |a, b| float_bin_op(a, b, FloatOps::min))
    }

    /// `f32x4.max` (Wasm semantics). Same lane function as scalar `f32.max`.
    pub fn f32x4_max(self, other: Self) -> Self {
        self.zip(other, |a, b| float_bin_op(a, b, FloatOps::max))
    }

    /// `f32x4.pmin`: `b < a ? b : a`.
    pub fn f32x4_pmin(self, other: Self) -> Self {
        self.zip(other, |a, b| if b < a { b } else { a })
    }

    /// `f32x4.pmax`: `a < b ? b : a`.
    pub fn f32x4_pmax(self, other: Self) -> Self {
        self.zip(other, |a, b| if a < b { b } else { a })
    }

    pub fn f32x4_eq(self, other: Self) -> Self {
        self.cmp(other, |a, b| a == b)
    }

    pub fn f32x4_ne(self, other: Self) -> Self {
        self.cmp(other, |a, b| a != b)
    }

    pub fn f32x4_lt(self, other: Self) -> Self {
        self.cmp(other, |a, b| a < b)
    }

    pub fn f32x4_gt(self, other: Self) -> Self {
        self.cmp(other, |a, b| a > b)
    }

    pub fn f32x4_le(self, other: Self) -> Self {
        self.cmp(other, |a, b| a <= b)
    }

    pub fn f32x4_ge(self, other: Self) -> Self {
        self.cmp(other, |a, b| a >= b)
    }

    // Nonstandard packed float math (0xE0-prefixed opcodes; opt-in via
    // `Extensions::ext_math`). Lane semantics are the Rust `f32` std
    // functions, so results are bit-identical to host code calling the same
    // functions on each lane.

    pub fn f32x4_sin(self) -> Self {
        self.map(|x| x.sin())
    }

    pub fn f32x4_cos(self) -> Self {
        self.map(|x| x.cos())
    }

    pub fn f32x4_tan(self) -> Self {
        self.map(|x| x.tan())
    }

    pub fn f32x4_asin(self) -> Self {
        self.map(|x| x.asin())
    }

    pub fn f32x4_acos(self) -> Self {
        self.map(|x| x.acos())
    }

    pub fn f32x4_atan(self) -> Self {
        self.map(|x| x.atan())
    }

    pub fn f32x4_exp(self) -> Self {
        self.map(|x| x.exp())
    }

    pub fn f32x4_ln(self) -> Self {
        self.map(|x| x.ln())
    }

    /// Lane-wise `a.atan2(b)` (first operand is `y`, second is `x`).
    pub fn f32x4_atan2(self, other: Self) -> Self {
        self.zip(other, |a, b| a.atan2(b))
    }

    /// Lane-wise `a.powf(b)`.
    pub fn f32x4_pow(self, other: Self) -> Self {
        self.zip(other, |a, b| a.powf(b))
    }

    /// Lane-wise Rust `f32::min` (minNum semantics: NaN loses). This is
    /// deliberately distinct from `f32x4.min` (Wasm semantics: NaN wins).
    pub fn f32x4_rmin(self, other: Self) -> Self {
        self.zip(other, |a, b| a.min(b))
    }

    /// Lane-wise Rust `f32::max`.
    pub fn f32x4_rmax(self, other: Self) -> Self {
        self.zip(other, |a, b| a.max(b))
    }

    /// Lane-wise Rust `%` (fmod).
    pub fn f32x4_rem(self, other: Self) -> Self {
        self.zip(other, |a, b| a % b)
    }

    /// Left-associated f32 dot product over the first `W` lanes:
    /// `((a0*b0 + a1*b1) + a2*b2) + a3*b3` — the exact operation order of
    /// the splash interpreter's `NumericValue::dot`.
    pub fn f32x4_dot<const W: usize>(self, other: Self) -> f32 {
        let a = self.to_f32x4();
        let b = other.to_f32x4();
        let mut sum = a[0] * b[0];
        for lane in 1..W {
            sum += a[lane] * b[lane];
        }
        sum
    }
}

/// Applies an infallible [`FloatOps`] unary operation to a lane.
fn float_op(x: f32, f: impl Fn(f32) -> Result<f32, crate::trap::Trap>) -> f32 {
    match f(x) {
        Ok(y) => y,
        // The float ops used here never trap.
        Err(_) => unreachable!(),
    }
}

/// Applies an infallible [`FloatOps`] binary operation to a lane pair.
fn float_bin_op(a: f32, b: f32, f: impl Fn(f32, f32) -> Result<f32, crate::trap::Trap>) -> f32 {
    match f(a, b) {
        Ok(y) => y,
        // The float ops used here never trap.
        Err(_) => unreachable!(),
    }
}
