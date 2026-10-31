//! The AIR builder for other front ends: languages that are not Splash
//! (the synth's formula modulators) lower their own syntax trees straight
//! into AIR with the Splash lowerer's builder (constant folding,
//! per-block CSE, memory-aware value reuse) and its portable f64 math
//! kernels (the same bits as `makepad_csg_math::portable`). No source text
//! passes between them.

use super::*;
use crate::ir::Stmt;

/// The portable f64 math a front end may call ([`AirBuilder::math`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathFn {
    Sin,
    Cos,
    Tan,
    Atan,
    /// `atan2(y, x)`.
    Atan2,
    Asin,
    Acos,
    Exp,
    /// The natural logarithm.
    Ln,
    Pow,
    Hypot,
    Cbrt,
}

impl MathFn {
    pub fn arity(self) -> usize {
        match self {
            MathFn::Atan2 | MathFn::Pow | MathFn::Hypot => 2,
            _ => 1,
        }
    }
}

/// Builds one AIR program. Blocks nest: [`AirBuilder::open`] (an `if`
/// arm) or [`AirBuilder::open_loop`] (a loop body), then
/// [`AirBuilder::close`] returns the statements for the [`Stmt::If`] or
/// [`Stmt::Loop`] the caller pushes.
pub struct AirBuilder {
    b: Builder,
}

impl Default for AirBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AirBuilder {
    pub fn new() -> Self {
        AirBuilder { b: Builder::new() }
    }

    /// Emits `op` of type `ty` (folded when its operands are constants,
    /// reused when the same op was emitted in an enclosing block).
    pub fn def(&mut self, ty: Ty, op: Op) -> Val {
        self.b.def(ty, op)
    }

    pub fn f64c(&mut self, x: f64) -> Val {
        self.b.cd(x)
    }

    pub fn i32c(&mut self, x: i32) -> Val {
        self.b.ci(x)
    }

    pub fn boolc(&mut self, x: bool) -> Val {
        self.b.cb(x)
    }

    /// The constant bits of `v`, when it is one.
    pub fn const_bits(&self, v: Val) -> Option<u64> {
        self.b.cst64(v)
    }

    pub fn ty(&self, v: Val) -> Ty {
        self.b.prog.vals[v.0 as usize]
    }

    pub fn var(&mut self, ty: Ty) -> Var {
        self.b.var(ty)
    }

    pub fn get(&mut self, v: Var) -> Val {
        self.b.get(v)
    }

    pub fn set(&mut self, v: Var, x: Val) {
        self.b.set(v, x)
    }

    /// `c ? x : y` (both already evaluated).
    pub fn sel(&mut self, c: Val, x: Val, y: Val) -> Val {
        self.b.sel(c, x, y)
    }

    pub fn load(&mut self, ty: Ty, region: Region, base: u32, extent: u32, off: Option<Val>) -> Val {
        self.b.load(ty, region, base, extent, off)
    }

    pub fn store(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>, val: Val) {
        self.b.store(region, base, extent, off, val)
    }

    /// Pushes a statement into the current block (`If`, `Loop`, `Break`,
    /// `Continue`; values and memory go through the methods above).
    pub fn push(&mut self, s: Stmt) {
        self.b.push(s)
    }

    pub fn open(&mut self) {
        self.b.open()
    }

    pub fn open_loop(&mut self) {
        self.b.open_loop()
    }

    pub fn close(&mut self) -> Block {
        self.b.close()
    }

    /// One portable math function of f64 arguments (an f64 result).
    pub fn math(&mut self, f: MathFn, args: &[Val]) -> Val {
        let b = &mut self.b;
        match f {
            MathFn::Sin => b.p_sin(args[0]),
            MathFn::Cos => b.p_cos(args[0]),
            MathFn::Tan => b.p_tan(args[0]),
            MathFn::Atan => b.p_atan(args[0]),
            MathFn::Atan2 => b.p_atan2(args[0], args[1]),
            MathFn::Asin => b.p_asin_acos(args[0], false),
            MathFn::Acos => b.p_asin_acos(args[0], true),
            MathFn::Exp => b.p_exp(args[0]),
            MathFn::Ln => b.p_ln(args[0]),
            MathFn::Pow => b.p_pow(args[0], args[1]),
            MathFn::Hypot => b.p_hypot(args[0], args[1]),
            MathFn::Cbrt => b.p_cbrt(args[0]),
        }
    }

    /// The finished program (`frame_words` frame scratch), with dead code
    /// removed. The caller validates it ([`ir::validate`]) against its
    /// regions before running or compiling it.
    pub fn finish(mut self, frame_words: u32) -> Program {
        let mut p = std::mem::take(&mut self.b.prog);
        p.body = self.b.blocks.into_iter().next().unwrap_or_default();
        p.frame_words = frame_words;
        dce(&mut p);
        p
    }
}
