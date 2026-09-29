//! The audio shader front end, part 2: static typing, whole-program
//! inlining and lowering of the AST into AIR ([`crate::ir`]).
//!
//! Front-end types: `f32`, `i32`, `bool`, `vec2` (a pair of f32 values)
//! and the aggregates (structs, fixed arrays), which always live in memory
//! (state, shared tables or the call frame) and are handled BY REFERENCE,
//! like Splash objects. Numeric literals stay untyped ([`V::Lit`]) until
//! they meet a typed value, so `i * 2` is integer and `x * 2` is float.
//!
//! Helpers are inlined at every call (monomorphized per call site); a
//! `return` inside one is a `Break` out of a one-iteration wrapper loop,
//! which is dropped again when the helper has no early return.

use crate::ir::{self, Bin, Block, Cmp, Op, Program, Region, Stmt as IS, Ty, Un, Val, Var};
use crate::parse::*;
use crate::ShaderError;
use std::collections::HashMap;

/// Words at the start of every state block (see `header`).
pub const HEADER_WORDS: u32 = 8;
pub mod header {
    pub const NOTE: u32 = 0;
    pub const FREQ: u32 = 1;
    pub const GATE: u32 = 2;
    pub const VELOCITY: u32 = 3;
    /// Absolute frame of the note-on (i32): `trigger` is true there.
    pub const TRIGGER: u32 = 4;
    /// Set to 1 by `stop()`.
    pub const STOP: u32 = 5;
    pub const RNG: u32 = 6;
    /// 1 until the first control block ran (params jump, `block()` runs).
    pub const FRESH: u32 = 7;
}
/// Ctx words: sample rate, the call's first absolute frame, then one
/// target word per parameter.
pub const CTX_RATE: u32 = 0;
pub const CTX_FRAME: u32 = 1;
pub const CTX_PARAMS: u32 = 2;

/// Frames per control block (params ramp across one; `block()` runs at
/// each start).
pub const CONTROL_BLOCK: u32 = 64;
/// Most frames one call may render.
pub const MAX_FRAMES: u32 = 4096;
/// Iteration cap of `while`/`loop` and of `for` loops with runtime bounds.
pub const LOOP_CAP: u32 = 1024;
/// `for` loops over constant ranges up to this many iterations (with no
/// break/continue of their own) are unrolled.
pub const UNROLL_MAX: u32 = 16;
pub const MAX_STATE_WORDS: u32 = 1 << 18;
pub const MAX_SHARED_WORDS: u32 = 1 << 22;
pub const MAX_FRAME_WORDS: u32 = 1 << 14;
/// Worst-case AIR ops per frame.
pub const MAX_COST_PER_FRAME: u64 = 4_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Instrument,
    Effect,
}

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub name: String,
    pub default: f32,
    pub min: f32,
    pub max: f32,
}

/// A state variable's place in the state block (for hot swap).
#[derive(Clone, Debug)]
pub struct StateVar {
    pub name: String,
    /// Type signature: equal signatures have equal layouts.
    pub sig: String,
    pub offset: u32,
    pub words: u32,
}

pub struct Lowered {
    pub kind: Kind,
    pub render: Program,
    pub init: Option<Program>,
    pub params: Vec<ParamInfo>,
    pub state_init: Vec<u32>,
    pub state_vars: Vec<StateVar>,
    pub shared_init: Vec<u32>,
}

// =========================================================================
// Types, values, places
// =========================================================================

#[derive(Clone, Debug, PartialEq)]
enum T {
    F,
    I,
    B,
    V2,
    Struct(usize),
    Array(Box<T>, u32),
}

#[derive(Clone, Debug)]
struct StructDef {
    name: String,
    /// (name, type, word offset, default init).
    fields: Vec<(String, T, u32, CInit)>,
    words: u32,
}

#[derive(Clone, Debug)]
struct Place {
    region: Region,
    root: u32,
    extent: u32,
    off: Option<Val>,
    stat: u32,
    ty: T,
}

#[derive(Clone, Debug)]
enum V {
    Lit(f64),
    F(Val),
    I(Val),
    B(Val),
    V2(Val, Val),
    Place(Place),
    Unit,
}

#[derive(Clone, Debug)]
enum Bind {
    Const(V),
    /// An immutable SSA value (loop indices).
    Value(V),
    /// A mutable scalar or vec2 local (or a promoted state scalar).
    Local(T, Vec<Var>),
    Place(Place),
    Param(usize),
}

/// A constant initializer (state, struct defaults, tables).
#[derive(Clone, Debug)]
enum CInit {
    W(u32, T),
    V2(u32, u32),
    Struct(usize, Vec<CInit>),
    Array(T, Vec<CInit>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LoopKind {
    User,
    Wrapper,
    Frame,
}

struct Ret {
    vars: Option<(T, Vec<Var>)>,
    used: bool,
}

type LResult<T> = Result<T, ShaderError>;

fn err<X>(span: Span, msg: impl Into<String>) -> LResult<X> {
    Err(ShaderError::new(span.start, span.end.max(span.start + 1), msg.into()))
}

// =========================================================================
// The builder: emits AIR with constant folding
// =========================================================================

#[derive(Default)]
struct Builder {
    prog: Program,
    blocks: Vec<Block>,
    consts: Vec<Option<u32>>,
}

impl Builder {
    fn new() -> Self {
        Builder { prog: Program::default(), blocks: vec![Vec::new()], consts: Vec::new() }
    }

    fn push(&mut self, s: IS) {
        self.blocks.last_mut().unwrap().push(s);
    }

    fn open(&mut self) {
        self.blocks.push(Vec::new());
    }

    fn close(&mut self) -> Block {
        self.blocks.pop().unwrap()
    }

    fn var(&mut self, ty: Ty) -> Var {
        self.prog.vars.push(ty);
        Var(self.prog.vars.len() as u32 - 1)
    }

    fn cst(&self, v: Val) -> Option<u32> {
        self.consts[v.0 as usize]
    }

    fn raw(&mut self, ty: Ty, op: Op, c: Option<u32>) -> Val {
        let v = Val(self.prog.vals.len() as u32);
        self.prog.vals.push(ty);
        self.consts.push(c);
        self.push(IS::Def(v, op));
        v
    }

    fn cf(&mut self, x: f32) -> Val {
        self.raw(Ty::F32, Op::ConstF(x), Some(x.to_bits()))
    }

    fn ci(&mut self, x: i32) -> Val {
        self.raw(Ty::I32, Op::ConstI(x), Some(x as u32))
    }

    fn cb(&mut self, x: bool) -> Val {
        self.raw(Ty::Bool, Op::ConstB(x), Some(x as u32))
    }

    fn konst(&mut self, ty: Ty, bits: u32) -> Val {
        match ty {
            Ty::F32 => self.cf(f32::from_bits(bits)),
            Ty::I32 => self.ci(bits as i32),
            Ty::Bool => self.cb(bits != 0),
        }
    }

    /// Emits `op` of type `ty`, folding it when every operand is constant.
    fn def(&mut self, ty: Ty, op: Op) -> Val {
        let folded = match op {
            Op::Un(u, a) => self.cst(a).map(|a| ir::eval_un(u, a)),
            Op::Bin(b, x, y) => match (self.cst(x), self.cst(y)) {
                (Some(x), Some(y)) => Some(ir::eval_bin(b, x, y)),
                _ => None,
            },
            Op::CmpF(cc, x, y) => match (self.cst(x), self.cst(y)) {
                (Some(x), Some(y)) => Some(ir::eval_cmp_f(cc, x, y)),
                _ => None,
            },
            Op::CmpI(cc, x, y) => match (self.cst(x), self.cst(y)) {
                (Some(x), Some(y)) => Some(ir::eval_cmp_i(cc, x, y)),
                _ => None,
            },
            Op::Sel(c, x, y) => match self.cst(c) {
                Some(c) => return if c != 0 { x } else { y },
                None => None,
            },
            Op::Wrap(x, len) => self.cst(x).map(|x| ir::eval_wrap(x, len)),
            _ => None,
        };
        match folded {
            Some(bits) => self.konst(ty, bits),
            None => self.raw(ty, op, None),
        }
    }

    fn un(&mut self, ty: Ty, u: Un, a: Val) -> Val {
        self.def(ty, Op::Un(u, a))
    }

    fn fb(&mut self, b: Bin, x: Val, y: Val) -> Val {
        self.def(Ty::F32, Op::Bin(b, x, y))
    }

    fn ib(&mut self, b: Bin, x: Val, y: Val) -> Val {
        self.def(Ty::I32, Op::Bin(b, x, y))
    }

    fn add(&mut self, x: Val, y: Val) -> Val {
        self.fb(Bin::AddF, x, y)
    }
    fn sub(&mut self, x: Val, y: Val) -> Val {
        self.fb(Bin::SubF, x, y)
    }
    fn mul(&mut self, x: Val, y: Val) -> Val {
        self.fb(Bin::MulF, x, y)
    }
    fn div(&mut self, x: Val, y: Val) -> Val {
        self.fb(Bin::DivF, x, y)
    }
    fn mulc(&mut self, x: Val, c: f32) -> Val {
        let c = self.cf(c);
        self.mul(x, c)
    }
    fn addc(&mut self, x: Val, c: f32) -> Val {
        let c = self.cf(c);
        self.add(x, c)
    }
    fn cmpf(&mut self, cc: Cmp, x: Val, y: Val) -> Val {
        self.def(Ty::Bool, Op::CmpF(cc, x, y))
    }
    fn cmpi(&mut self, cc: Cmp, x: Val, y: Val) -> Val {
        self.def(Ty::Bool, Op::CmpI(cc, x, y))
    }
    fn sel(&mut self, c: Val, x: Val, y: Val) -> Val {
        let ty = self.prog.vals[x.0 as usize];
        self.def(ty, Op::Sel(c, x, y))
    }
    fn get(&mut self, v: Var) -> Val {
        let ty = self.prog.vars[v.0 as usize];
        self.raw(ty, Op::Get(v), None)
    }
    fn set(&mut self, v: Var, x: Val) {
        self.push(IS::Set(v, x));
    }
    fn load(&mut self, ty: Ty, region: Region, base: u32, extent: u32, off: Option<Val>) -> Val {
        // A constant offset folds into the base.
        if let Some(o) = off {
            if let Some(c) = self.cst(o) {
                let c = ir::clamp_off(c, extent);
                return self.raw(ty, Op::Load { region, base: base + c, extent: 1, off: None }, None);
            }
        }
        self.raw(ty, Op::Load { region, base, extent, off }, None)
    }
    fn store(&mut self, region: Region, base: u32, extent: u32, off: Option<Val>, val: Val) {
        if let Some(o) = off {
            if let Some(c) = self.cst(o) {
                let c = ir::clamp_off(c, extent);
                self.push(IS::Store { region, base: base + c, extent: 1, off: None, val });
                return;
            }
        }
        self.push(IS::Store { region, base, extent, off, val });
    }

    /// True when a block only defines pure values (safe to hoist and run
    /// unconditionally).
    fn pure_block(b: &Block) -> bool {
        b.iter().all(|s| matches!(s, IS::Def(..)))
    }

    fn splice(&mut self, b: Block) {
        self.blocks.last_mut().unwrap().extend(b);
    }

    // -- deterministic math (Cephes-style polynomials, IEEE ops only) ----

    fn poly(&mut self, x: Val, coefs: &[f32]) -> Val {
        let mut acc = self.cf(coefs[0]);
        for c in &coefs[1..] {
            let m = self.mul(acc, x);
            acc = self.addc(m, *c);
        }
        acc
    }

    fn fabs(&mut self, x: Val) -> Val {
        self.un(Ty::F32, Un::AbsF, x)
    }

    fn fneg(&mut self, x: Val) -> Val {
        self.un(Ty::F32, Un::NegF, x)
    }

    /// sin (`cos == false`) or cos: octant reduction with a 3-part π/4,
    /// then the sin or cos minimax polynomial on [-π/4, π/4].
    fn sincos(&mut self, x: Val, cos: bool) -> Val {
        const FOPI: f32 = 1.273_239_5;
        const DP1: f32 = 0.785_156_25;
        const DP2: f32 = 2.418_756_5e-4;
        const DP3: f32 = 3.774_895e-8;
        let ax = self.fabs(x);
        let s = self.mulc(ax, FOPI);
        let j = self.un(Ty::I32, Un::F2I, s);
        let one = self.ci(1);
        let odd = self.ib(Bin::AndI, j, one);
        let j = self.ib(Bin::AddI, j, odd);
        let y = self.un(Ty::F32, Un::I2F, j);
        let seven = self.ci(7);
        let j = self.ib(Bin::AndI, j, seven);
        let three = self.ci(3);
        let flip = self.cmpi(Cmp::Gt, j, three);
        let four = self.ci(4);
        let jm4 = self.ib(Bin::SubI, j, four);
        let j = self.sel(flip, jm4, j);
        let t1 = self.mulc(y, DP1);
        let r = self.sub(ax, t1);
        let t2 = self.mulc(y, DP2);
        let r = self.sub(r, t2);
        let t3 = self.mulc(y, DP3);
        let r = self.sub(r, t3);
        let z = self.mul(r, r);
        // sin poly: ((s0 z + s1) z + s2) z r + r
        let sp = self.poly(z, &[-1.951_529_6e-4, 8.332_161e-3, -1.666_665_5e-1]);
        let sp = self.mul(sp, z);
        let sp = self.mul(sp, r);
        let sp = self.add(sp, r);
        // cos poly: ((c0 z + c1) z + c2) z z - 0.5 z + 1
        let cp = self.poly(z, &[2.443_315_7e-5, -1.388_731_6e-3, 4.166_664_6e-2]);
        let cp = self.mul(cp, z);
        let cp = self.mul(cp, z);
        let hz = self.mulc(z, 0.5);
        let cp = self.sub(cp, hz);
        let cp = self.addc(cp, 1.0);
        let is1 = self.cmpi(Cmp::Eq, j, one);
        let two = self.ci(2);
        let is2 = self.cmpi(Cmp::Eq, j, two);
        let mid = self.def(Ty::Bool, Op::Bin(Bin::OrB, is1, is2));
        let (v, neg) = if cos {
            let v = self.sel(mid, sp, cp);
            let gt1 = self.cmpi(Cmp::Gt, j, one);
            let neg = self.cmpi(Cmp::Ne, flip, gt1);
            (v, neg)
        } else {
            let v = self.sel(mid, cp, sp);
            let zero = self.cf(0.0);
            let xneg = self.cmpf(Cmp::Lt, x, zero);
            let neg = self.cmpi(Cmp::Ne, flip, xneg);
            (v, neg)
        };
        let nv = self.fneg(v);
        self.sel(neg, nv, v)
    }

    /// exp over [-87.3, 88]: 2^n by exponent bits times a degree-6
    /// polynomial on the reduced argument.
    fn exp(&mut self, x: Val) -> Val {
        let lo = self.cf(-87.3);
        let hi = self.cf(88.0);
        let x = self.fb(Bin::MaxF, x, lo);
        let x = self.fb(Bin::MinF, x, hi);
        let t = self.mulc(x, std::f32::consts::LOG2_E);
        let t = self.addc(t, 0.5);
        let fx = self.un(Ty::F32, Un::FloorF, t);
        let a = self.mulc(fx, 0.693_359_4);
        let x = self.sub(x, a);
        let b = self.mulc(fx, -2.121_944_4e-4);
        let x = self.sub(x, b);
        let z = self.mul(x, x);
        let p = self.poly(x, &[1.987_569_1e-4, 1.398_199_9e-3, 8.333_452e-3, 4.166_579_6e-2, 1.666_666_5e-1, 5.000_000_1e-1]);
        let p = self.mul(p, z);
        let p = self.add(p, x);
        let p = self.addc(p, 1.0);
        let n = self.un(Ty::I32, Un::F2I, fx);
        let c127 = self.ci(127);
        let n = self.ib(Bin::AddI, n, c127);
        let c23 = self.ci(23);
        let n = self.ib(Bin::ShlI, n, c23);
        let pow2 = self.un(Ty::F32, Un::BitsIF, n);
        self.mul(p, pow2)
    }

    /// Natural log for x > 0 (x <= 0 gives -inf).
    fn log(&mut self, x: Val) -> Val {
        let bits = self.un(Ty::I32, Un::BitsFI, x);
        let c23 = self.ci(23);
        let e = self.ib(Bin::ShrI, bits, c23);
        let c126 = self.ci(126);
        let e = self.ib(Bin::SubI, e, c126);
        let mask = self.ci(0x807f_ffffu32 as i32);
        let m = self.ib(Bin::AndI, bits, mask);
        let half = self.ci(0x3f00_0000);
        let m = self.ib(Bin::OrI, m, half);
        let m = self.un(Ty::F32, Un::BitsIF, m);
        let sqrthf = self.cf(std::f32::consts::FRAC_1_SQRT_2);
        let small = self.cmpf(Cmp::Lt, m, sqrthf);
        let one_i = self.ci(1);
        let em1 = self.ib(Bin::SubI, e, one_i);
        let e = self.sel(small, em1, e);
        let m2 = self.add(m, m);
        let a = self.addc(m2, -1.0);
        let b = self.addc(m, -1.0);
        let x1 = self.sel(small, a, b);
        let z = self.mul(x1, x1);
        let p = self.poly(
            x1,
            &[
                7.037_683_6e-2, -1.151_461e-1, 1.167_699_9e-1, -1.242_014_1e-1, 1.424_932_3e-1, -1.666_805_8e-1,
                2.000_071_4e-1, -2.499_999_4e-1, 3.333_333e-1,
            ],
        );
        let p = self.mul(p, x1);
        let y = self.mul(p, z);
        let fe = self.un(Ty::F32, Un::I2F, e);
        let t = self.mulc(fe, -2.121_944_4e-4);
        let y = self.add(y, t);
        let hz = self.mulc(z, 0.5);
        let y = self.sub(y, hz);
        let r = self.add(x1, y);
        let t = self.mulc(fe, 0.693_359_4);
        let r = self.add(r, t);
        let zero = self.cf(0.0);
        let pos = self.cmpf(Cmp::Gt, x, zero);
        let ninf = self.cf(f32::NEG_INFINITY);
        self.sel(pos, r, ninf)
    }

    fn tanh(&mut self, x: Val) -> Val {
        let z = self.fabs(x);
        let z2 = self.add(z, z);
        let e = self.exp(z2);
        let e1 = self.addc(e, 1.0);
        let two = self.cf(2.0);
        let q = self.div(two, e1);
        let one = self.cf(1.0);
        let big = self.sub(one, q);
        let nbig = self.fneg(big);
        let zero = self.cf(0.0);
        let xneg = self.cmpf(Cmp::Lt, x, zero);
        let big = self.sel(xneg, nbig, big);
        let s = self.mul(x, x);
        let p = self.poly(s, &[-5.704_988_7e-3, 2.063_908_9e-2, -5.373_971_6e-2, 1.333_144_2e-1, -3.333_328_2e-1]);
        let p = self.mul(p, s);
        let p = self.mul(p, x);
        let small = self.add(p, x);
        let lim = self.cf(0.625);
        let is_big = self.cmpf(Cmp::Gt, z, lim);
        self.sel(is_big, big, small)
    }

    fn pow(&mut self, a: Val, b: Val) -> Val {
        let l = self.log(a);
        let m = self.mul(b, l);
        let e = self.exp(m);
        let zero = self.cf(0.0);
        let pos = self.cmpf(Cmp::Gt, a, zero);
        self.sel(pos, e, zero)
    }

    fn floor_to_i(&mut self, x: Val) -> Val {
        let f = self.un(Ty::F32, Un::FloorF, x);
        self.un(Ty::I32, Un::F2I, f)
    }
}

/// Removes definitions nobody reads (every op is pure), to a fixpoint.
fn dce(p: &mut Program) {
    loop {
        let mut used = vec![false; p.vals.len()];
        fn mark(b: &Block, used: &mut Vec<bool>) {
            for s in b {
                let us: Vec<Val> = match s {
                    IS::Def(_, op) => ir::op_uses(op),
                    IS::Set(_, v) => vec![*v],
                    IS::Store { off, val, .. } => off.iter().copied().chain([*val]).collect(),
                    IS::Out { idx, val, .. } => vec![*idx, *val],
                    IS::If(c, t, e) => {
                        mark(t, used);
                        mark(e, used);
                        vec![*c]
                    }
                    IS::Loop { body, .. } => {
                        mark(body, used);
                        vec![]
                    }
                    _ => vec![],
                };
                for u in us {
                    used[u.0 as usize] = true;
                }
            }
        }
        mark(&p.body, &mut used);
        let mut removed = false;
        fn sweep(b: &mut Block, used: &[bool], removed: &mut bool) {
            b.retain(|s| match s {
                IS::Def(v, _) if !used[v.0 as usize] => {
                    *removed = true;
                    false
                }
                _ => true,
            });
            for s in b.iter_mut() {
                match s {
                    IS::If(_, t, e) => {
                        sweep(t, used, removed);
                        sweep(e, used, removed);
                    }
                    IS::Loop { body, .. } => sweep(body, used, removed),
                    _ => {}
                }
            }
        }
        sweep(&mut p.body, &used, &mut removed);
        if !removed {
            return;
        }
    }
}

// =========================================================================
// The lowerer
// =========================================================================

/// What the render entry exposes to expressions.
#[derive(Clone)]
struct EntryCtx {
    rate: Var,
    pos: Var,
    note: Var,
    freq: Var,
    gate: Var,
    velocity: Var,
    trigger_at: Var,
    rng: Var,
    stop: Var,
    params: Vec<(Var, Var)>,
}

struct Lowerer {
    b: Builder,
    structs: Vec<StructDef>,
    fns: HashMap<String, FnDecl>,
    globals: HashMap<String, Bind>,
    /// Per inlined call: a stack of lexical scopes.
    frames: Vec<Vec<HashMap<String, Bind>>>,
    loops: Vec<LoopKind>,
    rets: Vec<Ret>,
    call_stack: Vec<String>,
    params: Vec<ParamInfo>,
    state_init: Vec<u32>,
    state_vars: Vec<StateVar>,
    /// Promoted state scalars: (state word, var).
    promoted: Vec<(u32, Var)>,
    shared_init: Vec<u32>,
    frame_words: u32,
    entry: Option<EntryCtx>,
    /// Lowering `init()`: shared tables are writable, inputs absent.
    in_init: bool,
}

fn ty_of(t: &T) -> Ty {
    match t {
        T::F => Ty::F32,
        T::I => Ty::I32,
        T::B => Ty::Bool,
        _ => unreachable!("aggregate has no scalar type"),
    }
}

impl Lowerer {
    /// A throwaway builder for constant evaluation (sees the same vars).
    fn scratch(&self) -> Builder {
        let mut b = Builder::new();
        b.prog.vars = self.b.prog.vars.clone();
        b
    }

    fn words(&self, t: &T) -> u32 {
        match t {
            T::F | T::I | T::B => 1,
            T::V2 => 2,
            T::Struct(s) => self.structs[*s].words,
            T::Array(e, n) => self.words(e) * n,
        }
    }

    fn sig(&self, t: &T) -> String {
        match t {
            T::F => "f32".into(),
            T::I => "i32".into(),
            T::B => "bool".into(),
            T::V2 => "vec2".into(),
            T::Struct(s) => {
                let d = &self.structs[*s];
                let fields: Vec<String> = d.fields.iter().map(|(n, t, _, _)| format!("{}:{}", n, self.sig(t))).collect();
                format!("{}{{{}}}", d.name, fields.join(","))
            }
            T::Array(e, n) => format!("[{};{}]", self.sig(e), n),
        }
    }

    fn tname(&self, t: &T) -> String {
        match t {
            T::Struct(s) => self.structs[*s].name.clone(),
            T::Array(e, n) => format!("[{}; {}]", self.tname(e), n),
            _ => self.sig(t),
        }
    }

    fn vname(&self, v: &V) -> String {
        match v {
            V::Lit(_) | V::F(_) => "f32".into(),
            V::I(_) => "i32".into(),
            V::B(_) => "bool".into(),
            V::V2(..) => "vec2".into(),
            V::Place(p) => self.tname(&p.ty),
            V::Unit => "nothing".into(),
        }
    }

    // -- scopes ------------------------------------------------------------

    fn lookup(&self, name: &str) -> Option<Bind> {
        if let Some(frame) = self.frames.last() {
            for scope in frame.iter().rev() {
                if let Some(b) = scope.get(name) {
                    return Some(b.clone());
                }
            }
        }
        self.globals.get(name).cloned()
    }

    fn bind(&mut self, name: &str, b: Bind) {
        self.frames.last_mut().unwrap().last_mut().unwrap().insert(name.to_string(), b);
    }

    fn scoped<X>(&mut self, f: impl FnOnce(&mut Self) -> LResult<X>) -> LResult<X> {
        self.frames.last_mut().unwrap().push(HashMap::new());
        let r = f(self);
        self.frames.last_mut().unwrap().pop();
        r
    }

    fn alloc_frame(&mut self, words: u32, span: Span) -> LResult<u32> {
        let at = self.frame_words;
        self.frame_words += words;
        if self.frame_words > MAX_FRAME_WORDS {
            return err(span, format!("local arrays and structs exceed {} words; use a `var` (state) for big buffers", MAX_FRAME_WORDS));
        }
        Ok(at)
    }

    // -- value helpers -----------------------------------------------------

    fn to_f(&mut self, v: &V, span: Span) -> LResult<Val> {
        Ok(match v {
            V::Lit(x) => self.b.cf(*x as f32),
            V::F(x) => *x,
            V::I(x) => self.b.un(Ty::F32, Un::I2F, *x),
            _ => return err(span, format!("expected a number, found {}", self.vname(v))),
        })
    }

    fn to_i(&mut self, v: &V, span: Span) -> LResult<Val> {
        Ok(match v {
            V::Lit(x) => {
                if x.fract() != 0.0 {
                    return err(span, format!("{} is not an integer", x));
                }
                self.b.ci(*x as i32)
            }
            V::I(x) => *x,
            V::F(x) => self.b.floor_to_i(*x),
            _ => return err(span, format!("expected an integer, found {}", self.vname(v))),
        })
    }

    fn truth(&mut self, v: &V, span: Span) -> LResult<Val> {
        Ok(match v {
            V::B(x) => *x,
            V::Lit(x) => self.b.cb(*x != 0.0),
            V::F(x) => {
                let z = self.b.cf(0.0);
                self.b.cmpf(Cmp::Ne, *x, z)
            }
            V::I(x) => {
                let z = self.b.ci(0);
                self.b.cmpi(Cmp::Ne, *x, z)
            }
            _ => return err(span, format!("expected a condition, found {}", self.vname(v))),
        })
    }

    /// Reads a place of value type; aggregates stay places.
    fn read(&mut self, p: &Place) -> V {
        let (base, extent, off) = self.addr(p);
        match p.ty {
            T::F => V::F(self.b.load(Ty::F32, p.region, base, extent, off)),
            T::I => V::I(self.b.load(Ty::I32, p.region, base, extent, off)),
            T::B => V::B(self.b.load(Ty::Bool, p.region, base, extent, off)),
            T::V2 => {
                let x = self.b.load(Ty::F32, p.region, base, extent, off);
                let off1 = self.offset_plus(p, 1);
                let y = self.b.load(Ty::F32, p.region, off1.0, off1.1, off1.2);
                V::V2(x, y)
            }
            _ => V::Place(p.clone()),
        }
    }

    /// (base, extent, dynamic offset) of the place's first word.
    fn addr(&mut self, p: &Place) -> (u32, u32, Option<Val>) {
        self.offset_plus(p, 0)
    }

    fn offset_plus(&mut self, p: &Place, k: u32) -> (u32, u32, Option<Val>) {
        match p.off {
            None => (p.root + p.stat + k, 1, None),
            Some(o) => {
                let s = p.stat + k;
                let off = if s == 0 {
                    o
                } else {
                    let c = self.b.ci(s as i32);
                    self.b.ib(Bin::AddI, o, c)
                };
                (p.root, p.extent, Some(off))
            }
        }
    }

    fn write_word(&mut self, p: &Place, k: u32, val: Val) {
        let (base, extent, off) = self.offset_plus(p, k);
        self.b.store(p.region, base, extent, off, val);
    }

    /// Stores a value into a place of the same type (aggregates copy word
    /// by word).
    fn write(&mut self, p: &Place, v: &V, span: Span) -> LResult<()> {
        if p.region == Region::Shared && !self.in_init {
            return err(span, "tables (top-level `let` arrays) are read-only; fill them in `fn init()`");
        }
        match (&p.ty, v) {
            (T::F, _) => {
                let x = self.to_f(v, span)?;
                self.write_word(p, 0, x);
            }
            (T::I, V::I(_) | V::Lit(_)) => {
                let x = self.to_i(v, span)?;
                self.write_word(p, 0, x);
            }
            (T::B, V::B(x)) => self.write_word(p, 0, *x),
            (T::V2, _) => {
                let (x, y) = self.to_v2(v, span)?;
                self.write_word(p, 0, x);
                self.write_word(p, 1, y);
            }
            (t, V::Place(src)) if *t == src.ty => {
                let words = self.words(t);
                if words > 256 {
                    return err(span, "copying more than 256 words at once; copy the array element by element");
                }
                for k in 0..words {
                    let leaf = self.leaf_ty(t, k);
                    let (base, extent, off) = self.offset_plus(src, k);
                    let x = self.b.load(leaf, src.region, base, extent, off);
                    self.write_word(p, k, x);
                }
            }
            _ => return err(span, format!("cannot store {} into {}", self.vname(v), self.tname(&p.ty))),
        }
        Ok(())
    }

    /// The scalar type of word `k` of type `t`.
    fn leaf_ty(&self, t: &T, k: u32) -> Ty {
        match t {
            T::F | T::V2 => Ty::F32,
            T::I => Ty::I32,
            T::B => Ty::Bool,
            T::Struct(s) => {
                let d = &self.structs[*s];
                for (_, ft, off, _) in d.fields.iter().rev() {
                    if k >= *off {
                        return self.leaf_ty(ft, k - off);
                    }
                }
                unreachable!()
            }
            T::Array(e, _) => {
                let w = self.words(e);
                self.leaf_ty(e, k % w)
            }
        }
    }

    fn to_v2(&mut self, v: &V, span: Span) -> LResult<(Val, Val)> {
        match v {
            V::V2(x, y) => Ok((*x, *y)),
            V::Lit(_) | V::F(_) | V::I(_) => {
                let x = self.to_f(v, span)?;
                Ok((x, x))
            }
            _ => err(span, format!("expected a vec2, found {}", self.vname(v))),
        }
    }

    // -- constant initializers ---------------------------------------------

    fn cinit_ty(&self, c: &CInit) -> T {
        match c {
            CInit::W(_, t) => t.clone(),
            CInit::V2(..) => T::V2,
            CInit::Struct(s, _) => T::Struct(*s),
            CInit::Array(t, v) => T::Array(Box::new(t.clone()), v.len() as u32),
        }
    }

    fn flatten(c: &CInit, out: &mut Vec<u32>) {
        match c {
            CInit::W(w, _) => out.push(*w),
            CInit::V2(x, y) => {
                out.push(*x);
                out.push(*y);
            }
            CInit::Struct(_, f) | CInit::Array(_, f) => {
                for c in f {
                    Self::flatten(c, out);
                }
            }
        }
    }

    /// Evaluates a constant initializer (a scratch builder folds every
    /// scalar, so the deterministic math works in initializers too).
    fn cinit(&mut self, e: &Expr, ann: Option<&TypeAnn>) -> LResult<CInit> {
        match &e.kind {
            ExprKind::StructLit(name, fields) => {
                let Some(s) = self.structs.iter().position(|d| d.name == *name) else {
                    return err(e.span, format!("unknown struct `{}`", name));
                };
                let mut vals: Vec<CInit> = self.structs[s].fields.iter().map(|f| f.3.clone()).collect();
                for (fname, fe) in fields {
                    let Some(k) = self.structs[s].fields.iter().position(|f| f.0 == *fname) else {
                        return err(fe.span, format!("`{}` has no field `{}`", name, fname));
                    };
                    let v = self.cinit(fe, None)?;
                    let want = self.structs[s].fields[k].1.clone();
                    vals[k] = self.coerce_cinit(v, &want, fe.span)?;
                }
                Ok(CInit::Struct(s, vals))
            }
            ExprKind::ArrayRepeat(el, count) => {
                let n = self.const_count(count)?;
                let el = self.cinit(el, ann)?;
                let t = self.cinit_ty(&el);
                Ok(CInit::Array(t, vec![el; n as usize]))
            }
            ExprKind::ArrayList(list) => {
                let mut out = Vec::new();
                for x in list {
                    out.push(self.cinit(x, ann)?);
                }
                let t = self.cinit_ty(&out[0]);
                for (x, c) in list.iter().zip(out.iter_mut()) {
                    let owned = c.clone();
                    *c = self.coerce_cinit(owned, &t, x.span)?;
                }
                Ok(CInit::Array(t, out))
            }
            _ => {
                let mut scratch = self.scratch();
                std::mem::swap(&mut self.b, &mut scratch);
                let saved_entry = self.entry.take();
                let saved_frame = self.frame_words;
                let v2 = self.expr(e);
                std::mem::swap(&mut self.b, &mut scratch);
                self.entry = saved_entry;
                self.frame_words = saved_frame;
                let v2 = v2?;
                let scratch_const = |v: &V, b: &Builder| -> Option<u32> {
                    match v {
                        V::F(x) | V::I(x) | V::B(x) => b.cst(*x),
                        _ => None,
                    }
                };
                let bad = || err(e.span, "initial values must be constants (numbers, constants, math on them)");
                Ok(match (&v2, ann) {
                    (V::Lit(x), Some(TypeAnn::I32)) => {
                        if x.fract() != 0.0 {
                            return err(e.span, "not an integer");
                        }
                        CInit::W(*x as i32 as u32, T::I)
                    }
                    (V::Lit(x), Some(TypeAnn::Vec2)) => {
                        let b = (*x as f32).to_bits();
                        CInit::V2(b, b)
                    }
                    (V::Lit(x), _) => CInit::W((*x as f32).to_bits(), T::F),
                    (V::F(_), _) => CInit::W(scratch_const(&v2, &scratch).map_or_else(bad, Ok)?, T::F),
                    (V::I(_), _) => CInit::W(scratch_const(&v2, &scratch).map_or_else(bad, Ok)?, T::I),
                    (V::B(_), _) => CInit::W(scratch_const(&v2, &scratch).map_or_else(bad, Ok)?, T::B),
                    (V::V2(x, y), _) => match (scratch.cst(*x), scratch.cst(*y)) {
                        (Some(x), Some(y)) => CInit::V2(x, y),
                        _ => return err(e.span, "initial values must be constants (numbers, constants, math on them)"),
                    },
                    _ => return err(e.span, "initial values must be constants (numbers, constants, math on them)"),
                })
            }
        }
    }

    fn coerce_cinit(&self, c: CInit, want: &T, span: Span) -> LResult<CInit> {
        let have = self.cinit_ty(&c);
        if have == *want {
            return Ok(c);
        }
        match (c, want) {
            (CInit::W(w, T::F), T::I) => {
                let x = f32::from_bits(w);
                if x.fract() != 0.0 {
                    return err(span, "expected an integer");
                }
                Ok(CInit::W(x as i32 as u32, T::I))
            }
            (CInit::W(w, T::I), T::F) => Ok(CInit::W((w as i32 as f32).to_bits(), T::F)),
            (CInit::W(w, T::F), T::V2) => Ok(CInit::V2(w, w)),
            _ => err(span, format!("expected {}, found {}", self.tname(want), self.tname(&have))),
        }
    }

    fn const_count(&mut self, e: &Expr) -> LResult<u32> {
        let scratch = self.scratch();
        let saved = std::mem::replace(&mut self.b, scratch);
        let r = self.expr(e);
        self.b = saved;
        match r? {
            V::Lit(x) if x.fract() == 0.0 && x >= 1.0 && x <= (1u64 << 24) as f64 => Ok(x as u32),
            _ => err(e.span, "array sizes must be positive integer constants (`let N = 64`)"),
        }
    }

    /// Writes a constant initializer into a place (frame locals).
    fn store_cinit(&mut self, p: &Place, c: &CInit) {
        let mut words = Vec::new();
        Self::flatten(c, &mut words);
        let t = self.cinit_ty(c);
        let n = words.len() as u32;
        let uniform = words.iter().all(|w| *w == words[0]);
        if n > 8 && uniform {
            // A fill loop instead of n stores.
            let leaf = self.leaf_ty(&t, 0);
            let j = self.b.var(Ty::I32);
            let zero = self.b.ci(0);
            self.b.set(j, zero);
            self.b.open();
            let jv = self.b.get(j);
            let end = self.b.ci(n as i32);
            let done = self.b.cmpi(Cmp::Ge, jv, end);
            self.b.push(IS::If(done, vec![IS::Break(0)], vec![]));
            let x = self.b.konst(leaf, words[0]);
            let (base, extent, off) = match p.off {
                None => (p.root + p.stat, n, Some(jv)),
                Some(_) => {
                    let (b, e, o) = self.offset_plus(p, 0);
                    let o = self.b.ib(Bin::AddI, o.unwrap(), jv);
                    (b, e, Some(o))
                }
            };
            self.b.push(IS::Store { region: p.region, base, extent, off, val: x });
            let one = self.b.ci(1);
            let jn = self.b.ib(Bin::AddI, jv, one);
            self.b.set(j, jn);
            let body = self.b.close();
            self.b.push(IS::Loop { cap: n + 1, body });
            return;
        }
        for (k, w) in words.iter().enumerate() {
            let leaf = self.leaf_ty(&t, k as u32);
            let x = self.b.konst(leaf, *w);
            self.write_word(p, k as u32, x);
        }
    }

    // -- top level -----------------------------------------------------------

    fn top(&mut self, items: &[Item]) -> LResult<()> {
        for item in items {
            if let Item::Fn(f) = item {
                if self.fns.insert(f.name.clone(), f.clone()).is_some() {
                    return err(f.span, format!("`{}` is defined twice", f.name));
                }
            }
        }
        // Header words.
        self.state_init = vec![0; HEADER_WORDS as usize];
        self.state_init[header::FRESH as usize] = 1;
        self.state_init[header::RNG as usize] = 0x9e37_79b9;
        for item in items {
            match item {
                Item::Struct { name, fields, span } => {
                    if self.structs.iter().any(|s| s.name == *name) {
                        return err(*span, format!("struct `{}` is defined twice", name));
                    }
                    let mut defs = Vec::new();
                    let mut off = 0;
                    for (fname, fe) in fields {
                        let c = self.cinit(fe, None)?;
                        let t = self.cinit_ty(&c);
                        let w = self.words(&t);
                        defs.push((fname.clone(), t, off, c));
                        off += w;
                    }
                    self.structs.push(StructDef { name: name.clone(), fields: defs, words: off.max(1) });
                    if off == 0 {
                        return err(*span, "a struct needs at least one field");
                    }
                }
                Item::Let { name, ann, value, span } => {
                    self.check_new_global(name, *span)?;
                    if let ExprKind::Call(f, args) = &value.kind {
                        if f == "param" {
                            let bind = self.param(name, args, value.span)?;
                            self.globals.insert(name.clone(), bind);
                            continue;
                        }
                    }
                    if matches!(value.kind, ExprKind::ArrayRepeat(..) | ExprKind::ArrayList(..) | ExprKind::StructLit(..)) {
                        let c = self.cinit(value, ann.as_ref())?;
                        let t = self.cinit_ty(&c);
                        let root = self.shared_init.len() as u32;
                        Self::flatten(&c, &mut self.shared_init);
                        if self.shared_init.len() as u32 > MAX_SHARED_WORDS {
                            return err(*span, "tables exceed 4 Mi words");
                        }
                        let extent = self.words(&t);
                        self.globals.insert(
                            name.clone(),
                            Bind::Place(Place { region: Region::Shared, root, extent, off: None, stat: 0, ty: t }),
                        );
                        continue;
                    }
                    let scratch = self.scratch();
        let saved = std::mem::replace(&mut self.b, scratch);
                    let r = self.expr(value);
                    self.b = saved;
                    match r? {
                        v @ V::Lit(_) => {
                            self.globals.insert(name.clone(), Bind::Const(v));
                        }
                        _ => {
                            // A folded non-literal constant (e.g. `sin(1)`).
                            let c = self.cinit(value, ann.as_ref())?;
                            let v = match c {
                                CInit::W(w, T::F) => V::Lit(f32::from_bits(w) as f64),
                                CInit::W(w, T::I) => V::Lit(w as i32 as f64),
                                _ => return err(*span, "a top-level `let` must be a constant, a `param(...)` or a table; use `var` for state"),
                            };
                            self.globals.insert(name.clone(), Bind::Const(v));
                        }
                    }
                }
                Item::Var { name, ann, value, span } => {
                    self.check_new_global(name, *span)?;
                    let c = self.cinit(value, ann.as_ref())?;
                    let t = self.cinit_ty(&c);
                    let offset = self.state_init.len() as u32;
                    Self::flatten(&c, &mut self.state_init);
                    if self.state_init.len() as u32 > MAX_STATE_WORDS {
                        return err(*span, format!("state exceeds {} words (1 MiB) per instance", MAX_STATE_WORDS));
                    }
                    let words = self.words(&t);
                    self.state_vars.push(StateVar { name: name.clone(), sig: self.sig(&t), offset, words });
                    let bind = match t {
                        T::F | T::I | T::B => {
                            let v = self.b.var(ty_of(&t));
                            self.promoted.push((offset, v));
                            Bind::Local(t, vec![v])
                        }
                        T::V2 => {
                            let x = self.b.var(Ty::F32);
                            let y = self.b.var(Ty::F32);
                            self.promoted.push((offset, x));
                            self.promoted.push((offset + 1, y));
                            Bind::Local(T::V2, vec![x, y])
                        }
                        t => Bind::Place(Place { region: Region::State, root: offset, extent: words, off: None, stat: 0, ty: t }),
                    };
                    self.globals.insert(name.clone(), bind);
                }
                Item::Fn(_) => {}
            }
        }
        Ok(())
    }

    fn check_new_global(&self, name: &str, span: Span) -> LResult<()> {
        if self.globals.contains_key(name) {
            return err(span, format!("`{}` is defined twice", name));
        }
        if BUILTIN_NAMES.contains(&name) {
            return err(span, format!("`{}` is a builtin name", name));
        }
        Ok(())
    }

    fn param(&mut self, name: &str, args: &[Expr], span: Span) -> LResult<Bind> {
        let mut nums = Vec::new();
        for a in args {
            let scratch = self.scratch();
        let saved = std::mem::replace(&mut self.b, scratch);
            let r = self.expr(a);
            self.b = saved;
            match r? {
                V::Lit(x) => nums.push(x as f32),
                _ => return err(a.span, "param(default, min, max) takes constant numbers"),
            }
        }
        let (default, min, max) = match nums[..] {
            [d] => (d, f32::MIN, f32::MAX),
            [d, lo, hi] => (d, lo, hi),
            _ => return err(span, "param(default) or param(default, min, max)"),
        };
        if self.params.len() >= 64 {
            return err(span, "at most 64 params");
        }
        self.params.push(ParamInfo { name: name.to_string(), default, min, max });
        Ok(Bind::Param(self.params.len() - 1))
    }

    // -- entries -------------------------------------------------------------

    fn load_promoted(&mut self) {
        for (word, var) in self.promoted.clone() {
            let ty = self.b.prog.vars[var.0 as usize];
            let x = self.b.load(ty, Region::State, word, 1, None);
            self.b.set(var, x);
        }
    }

    fn store_promoted(&mut self) {
        for (word, var) in self.promoted.clone() {
            let x = self.b.get(var);
            self.b.store(Region::State, word, 1, None, x);
        }
    }

    fn header_var(&mut self, ty: Ty, word: u32, store_back: bool) -> Var {
        let v = self.b.var(ty);
        if store_back {
            self.promoted.push((word, v));
        } else {
            let x = self.b.load(ty, Region::State, word, 1, None);
            self.b.set(v, x);
        }
        v
    }

    fn render(&mut self, kind: Kind) -> LResult<()> {
        let entry_name = if kind == Kind::Instrument { "voice" } else { "effect" };
        let entry = self.fns.get(entry_name).cloned().unwrap();
        let fresh_word = header::FRESH;
        // Header and param smoothing words are promoted like state scalars.
        let rng = self.header_var(Ty::I32, header::RNG, true);
        let stop = self.header_var(Ty::I32, header::STOP, true);
        let fresh = self.header_var(Ty::I32, fresh_word, true);
        let mut params = Vec::new();
        for k in 0..self.params.len() as u32 {
            let base = self.header_var(Ty::F32, HEADER_WORDS + 2 * k, true);
            let step = self.header_var(Ty::F32, HEADER_WORDS + 2 * k + 1, true);
            params.push((base, step));
        }
        self.load_promoted();
        let note = self.header_var(Ty::F32, header::NOTE, false);
        // freq from note with the deterministic exp (not the host's libm).
        let freq = self.b.var(Ty::F32);
        let nv = self.b.get(note);
        let d = self.b.addc(nv, -69.0);
        let y = self.b.mulc(d, std::f32::consts::LN_2 / 12.0);
        let e = self.b.exp(y);
        let hz = self.b.mulc(e, 440.0);
        self.b.set(freq, hz);
        let gate = self.header_var(Ty::F32, header::GATE, false);
        let velocity = self.header_var(Ty::F32, header::VELOCITY, false);
        let trigger_at = self.header_var(Ty::I32, header::TRIGGER, false);
        let rate = self.b.var(Ty::F32);
        let r = self.b.load(Ty::F32, Region::Ctx, CTX_RATE, 1, None);
        self.b.set(rate, r);
        let frame0 = self.b.load(Ty::I32, Region::Ctx, CTX_FRAME, 1, None);
        let n = self.b.raw(Ty::I32, Op::FrameCount, None);
        let i = self.b.var(Ty::I32);
        let zero = self.b.ci(0);
        self.b.set(i, zero);
        let pos = self.b.var(Ty::I32);
        self.entry = Some(EntryCtx { rate, pos, note, freq, gate, velocity, trigger_at, rng, stop, params: params.clone() });

        // The frame loop.
        self.b.open();
        self.loops.push(LoopKind::Frame);
        let iv = self.b.get(i);
        let done = self.b.cmpi(Cmp::Ge, iv, n);
        self.b.push(IS::If(done, vec![IS::Break(0)], vec![]));
        let p = self.b.ib(Bin::AddI, frame0, iv);
        self.b.set(pos, p);
        let mask = self.b.ci(CONTROL_BLOCK as i32 - 1);
        let phase = self.b.ib(Bin::AndI, p, mask);
        let at_start = self.b.cmpi(Cmp::Eq, phase, zero);
        let fr = self.b.get(fresh);
        let is_fresh = self.b.cmpi(Cmp::Ne, fr, zero);
        let run_block = self.b.def(Ty::Bool, Op::Bin(Bin::OrB, at_start, is_fresh));
        // Control block start: params ramp toward their targets; block().
        self.b.open();
        for (k, (base, step)) in params.iter().enumerate() {
            let target = self.b.load(Ty::F32, Region::Ctx, CTX_PARAMS + k as u32, 1, None);
            let bv = self.b.get(*base);
            let sv = self.b.get(*step);
            let s64 = self.b.mulc(sv, CONTROL_BLOCK as f32);
            let reached = self.b.add(bv, s64);
            let nb = self.b.sel(is_fresh, target, reached);
            self.b.set(*base, nb);
            let d = self.b.sub(target, nb);
            let ns = self.b.mulc(d, 1.0 / CONTROL_BLOCK as f32);
            let zf = self.b.cf(0.0);
            let ns = self.b.sel(is_fresh, zf, ns);
            self.b.set(*step, ns);
        }
        self.b.set(fresh, zero);
        if let Some(block_fn) = self.fns.get("block").cloned() {
            if !block_fn.params.is_empty() {
                return err(block_fn.span, "`fn block()` takes no parameters");
            }
            let v = self.call(&block_fn, vec![], block_fn.span)?;
            let _ = v;
        }
        let blk = self.b.close();
        self.b.push(IS::If(run_block, blk, vec![]));

        // The sample.
        let args = match kind {
            Kind::Instrument => {
                if !entry.params.is_empty() {
                    return err(entry.span, "`fn voice()` takes no parameters (use note, freq, gate, velocity)");
                }
                vec![]
            }
            Kind::Effect => {
                if entry.params.len() != 2 {
                    return err(entry.span, "`fn effect(l, r)` takes the left and right input");
                }
                let l = self.b.raw(Ty::F32, Op::In { ch: 0, idx: iv }, None);
                let r = self.b.raw(Ty::F32, Op::In { ch: 1, idx: iv }, None);
                vec![V::F(l), V::F(r)]
            }
        };
        let out = self.call(&entry, args, entry.span)?;
        let (l, r) = match &out {
            V::Unit => return err(entry.span, format!("`fn {}` must return the output sample (f32 or vec2)", entry_name)),
            v => self.to_v2(v, entry.span)?,
        };
        let iv2 = self.b.get(i);
        self.b.push(IS::Out { ch: 0, idx: iv2, val: l });
        self.b.push(IS::Out { ch: 1, idx: iv2, val: r });
        let one = self.b.ci(1);
        let inext = self.b.ib(Bin::AddI, iv2, one);
        self.b.set(i, inext);
        self.loops.pop();
        let body = self.b.close();
        self.b.push(IS::Loop { cap: MAX_FRAMES + 1, body });
        self.store_promoted();
        Ok(())
    }

    fn init_program(&mut self, f: &FnDecl) -> LResult<Program> {
        let saved_b = std::mem::replace(&mut self.b, Builder::new());
        let saved_frame = std::mem::replace(&mut self.frame_words, 0);
        let saved_entry = self.entry.take();
        // State `var`s are per instance: init sees only tables and constants.
        let saved_globals = self.globals.clone();
        self.globals.retain(|_, b| !matches!(b, Bind::Local(..)) && !matches!(b, Bind::Place(p) if p.region == Region::State));
        self.in_init = true;
        if !f.params.is_empty() {
            return err(f.span, "`fn init()` takes no parameters");
        }
        let r = self.call(f, vec![], f.span);
        self.in_init = false;
        self.globals = saved_globals;
        self.entry = saved_entry;
        let frame_words = std::mem::replace(&mut self.frame_words, saved_frame);
        let b = std::mem::replace(&mut self.b, saved_b);
        r?;
        let mut prog = b.prog;
        prog.body = b.blocks.into_iter().next().unwrap();
        prog.frame_words = frame_words;
        dce(&mut prog);
        Ok(prog)
    }

    // -- calls (inlining) ------------------------------------------------------

    fn call(&mut self, f: &FnDecl, args: Vec<V>, span: Span) -> LResult<V> {
        if self.call_stack.iter().any(|n| *n == f.name) {
            return err(span, format!("`{}` calls itself; audio shaders have no recursion", f.name));
        }
        if args.len() != f.params.len() {
            return err(span, format!("`{}` takes {} arguments, got {}", f.name, f.params.len(), args.len()));
        }
        let mut scope = HashMap::new();
        for ((name, ann), a) in f.params.iter().zip(args) {
            let bind = match (a, ann) {
                (V::Place(p), _) => Bind::Place(p),
                (V::Lit(x), None) => Bind::Const(V::Lit(x)),
                (v, ann) => {
                    let v = match ann {
                        Some(TypeAnn::F32) => V::F(self.to_f(&v, span)?),
                        Some(TypeAnn::I32) => V::I(self.to_i(&v, span)?),
                        Some(TypeAnn::Vec2) => {
                            let (x, y) = self.to_v2(&v, span)?;
                            V::V2(x, y)
                        }
                        _ => match v {
                            V::Lit(x) => V::F(self.b.cf(x as f32)),
                            v => v,
                        },
                    };
                    self.new_local(&v, span)?
                }
            };
            scope.insert(name.clone(), bind);
        }
        self.call_stack.push(f.name.clone());
        self.frames.push(vec![scope]);
        self.rets.push(Ret { vars: None, used: false });
        self.loops.push(LoopKind::Wrapper);
        self.b.open();
        let r = self.stmts(&f.body);
        let body = self.b.close();
        self.loops.pop();
        let ret = self.rets.pop().unwrap();
        self.frames.pop();
        self.call_stack.pop();
        let v = r?;
        if !ret.used {
            self.b.splice(body);
            return Ok(v);
        }
        let (t, vars) = ret.vars.clone().unwrap_or((T::F, vec![]));
        let mut body = body;
        // The fall-through value (if any) is the last return.
        if !matches!(body.last(), Some(IS::Break(_))) {
            if vars.is_empty() {
                if !matches!(v, V::Unit) {
                    return err(span, format!("`{}` returns a value in some paths only", f.name));
                }
            } else {
                if matches!(v, V::Unit) {
                    return err(f.span, format!("`{}` must end with a value (its `return`s give one)", f.name));
                }
                self.b.open();
                self.assign_vars(&t, &vars, &v, f.span)?;
                let tail = self.b.close();
                body.extend(tail);
            }
        }
        self.zero_vars(&vars);
        self.b.push(IS::Loop { cap: 1, body });
        if vars.is_empty() {
            return Ok(V::Unit);
        }
        Ok(self.read_vars(&t, &vars))
    }

    fn zero_vars(&mut self, vars: &[Var]) {
        for v in vars {
            let ty = self.b.prog.vars[v.0 as usize];
            let z = self.b.konst(ty, 0);
            self.b.set(*v, z);
        }
    }

    fn new_local(&mut self, v: &V, span: Span) -> LResult<Bind> {
        let (t, vars) = match v {
            V::Lit(_) | V::F(_) => (T::F, vec![self.b.var(Ty::F32)]),
            V::I(_) => (T::I, vec![self.b.var(Ty::I32)]),
            V::B(_) => (T::B, vec![self.b.var(Ty::Bool)]),
            V::V2(..) => (T::V2, vec![self.b.var(Ty::F32), self.b.var(Ty::F32)]),
            V::Place(p) => return Ok(Bind::Place(p.clone())),
            V::Unit => return err(span, "this has no value"),
        };
        self.assign_vars(&t, &vars, v, span)?;
        Ok(Bind::Local(t, vars))
    }

    fn assign_vars(&mut self, t: &T, vars: &[Var], v: &V, span: Span) -> LResult<()> {
        match t {
            T::F => {
                let x = self.to_f(v, span)?;
                self.b.set(vars[0], x);
            }
            T::I => {
                let x = match v {
                    V::I(_) | V::Lit(_) => self.to_i(v, span)?,
                    _ => return err(span, format!("expected i32, found {} (use int(x))", self.vname(v))),
                };
                self.b.set(vars[0], x);
            }
            T::B => {
                let V::B(x) = v else { return err(span, format!("expected bool, found {}", self.vname(v))) };
                self.b.set(vars[0], *x);
            }
            T::V2 => {
                let (x, y) = self.to_v2(v, span)?;
                self.b.set(vars[0], x);
                self.b.set(vars[1], y);
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn read_vars(&mut self, t: &T, vars: &[Var]) -> V {
        match t {
            T::F => V::F(self.b.get(vars[0])),
            T::I => V::I(self.b.get(vars[0])),
            T::B => V::B(self.b.get(vars[0])),
            T::V2 => {
                let x = self.b.get(vars[0]);
                let y = self.b.get(vars[1]);
                V::V2(x, y)
            }
            _ => unreachable!(),
        }
    }

    /// The type a value would take in a fresh variable.
    fn value_t(v: &V) -> Option<T> {
        Some(match v {
            V::Lit(_) | V::F(_) => T::F,
            V::I(_) => T::I,
            V::B(_) => T::B,
            V::V2(..) => T::V2,
            _ => return None,
        })
    }

    // -- statements ------------------------------------------------------------

    /// Lowers statements; the value is the last expression statement's.
    fn stmts(&mut self, stmts: &[Stmt]) -> LResult<V> {
        let mut last = V::Unit;
        for (k, s) in stmts.iter().enumerate() {
            last = V::Unit;
            match s {
                Stmt::Let { name, ann, value, span } => {
                    let v = self.expr(value)?;
                    let v = match ann {
                        Some(TypeAnn::I32) => V::I(self.to_i(&v, *span)?),
                        Some(TypeAnn::F32) => V::F(self.to_f(&v, *span)?),
                        Some(TypeAnn::Vec2) => {
                            let (x, y) = self.to_v2(&v, *span)?;
                            V::V2(x, y)
                        }
                        _ => v,
                    };
                    let bind = self.new_local(&v, *span)?;
                    self.bind(name, bind);
                }
                Stmt::Assign { target, op, value, span } => self.assign(target, *op, value, *span)?,
                Stmt::Expr(e) => {
                    let v = self.expr(e)?;
                    if k + 1 == stmts.len() {
                        last = v;
                    }
                }
                Stmt::For { var, from, to, body, span } => self.for_loop(var, from, to, body, *span)?,
                Stmt::While { cond, body, .. } => {
                    self.b.open();
                    self.loops.push(LoopKind::User);
                    let r = (|| {
                        let c = self.expr(cond)?;
                        let c = self.truth(&c, cond.span)?;
                        let nc = self.b.un(Ty::Bool, Un::NotB, c);
                        self.b.push(IS::If(nc, vec![IS::Break(0)], vec![]));
                        self.scoped(|l| l.stmts(body))
                    })();
                    self.loops.pop();
                    let b = self.b.close();
                    r?;
                    self.b.push(IS::Loop { cap: LOOP_CAP, body: b });
                }
                Stmt::Loop { body, .. } => {
                    self.b.open();
                    self.loops.push(LoopKind::User);
                    let r = self.scoped(|l| l.stmts(body));
                    self.loops.pop();
                    let b = self.b.close();
                    r?;
                    self.b.push(IS::Loop { cap: LOOP_CAP, body: b });
                }
                Stmt::Break(span) | Stmt::Continue(span) => {
                    let mut depth = 0;
                    let mut found = false;
                    for kind in self.loops.iter().rev() {
                        match kind {
                            LoopKind::User => {
                                found = true;
                                break;
                            }
                            LoopKind::Wrapper => depth += 1,
                            LoopKind::Frame => break,
                        }
                    }
                    // A wrapper between us and the loop means the loop is
                    // in a caller: not reachable.
                    let wrapper_between = self
                        .loops
                        .iter()
                        .rev()
                        .take_while(|k| **k != LoopKind::User)
                        .any(|k| *k == LoopKind::Wrapper);
                    if !found || wrapper_between {
                        return err(*span, "`break`/`continue` outside a loop");
                    }
                    self.b.push(if matches!(s, Stmt::Break(_)) { IS::Break(depth) } else { IS::Continue(depth) });
                }
                Stmt::Return(value, span) => {
                    let v = match value {
                        Some(e) => self.expr(e)?,
                        None => V::Unit,
                    };
                    let depth = self.loops.iter().rev().position(|k| *k == LoopKind::Wrapper).unwrap() as u32;
                    let ret_i = self.rets.len() - 1;
                    if !matches!(v, V::Unit) {
                        if self.rets[ret_i].vars.is_none() {
                            let Some(t) = Self::value_t(&v) else {
                                return err(*span, "functions return f32, i32, bool or vec2 (aggregates are passed by reference instead)");
                            };
                            let vars = match &t {
                                T::V2 => vec![self.b.var(Ty::F32), self.b.var(Ty::F32)],
                                t => vec![self.b.var(ty_of(t))],
                            };
                            self.rets[ret_i].vars = Some((t, vars));
                        }
                        let (t, vars) = self.rets[ret_i].vars.clone().unwrap();
                        self.assign_vars(&t, &vars, &v, *span)?;
                    } else if self.rets[ret_i].vars.is_some() {
                        return err(*span, "this `return` needs a value");
                    }
                    self.rets[ret_i].used = true;
                    self.b.push(IS::Break(depth));
                    // Anything after a return in this block is dead.
                    return Ok(V::Unit);
                }
            }
        }
        Ok(last)
    }

    fn assign(&mut self, target: &Expr, op: AssignOp, value: &Expr, span: Span) -> LResult<()> {
        let rhs = self.expr(value)?;
        let combine = |l: &mut Self, old: V| -> LResult<V> {
            let bop = match op {
                AssignOp::Set => return Ok(rhs.clone()),
                AssignOp::Add => BinOp::Add,
                AssignOp::Sub => BinOp::Sub,
                AssignOp::Mul => BinOp::Mul,
                AssignOp::Div => BinOp::Div,
                AssignOp::Rem => BinOp::Rem,
            };
            l.binary(bop, old, rhs.clone(), span)
        };
        match &target.kind {
            ExprKind::Ident(name) => match self.lookup(name) {
                Some(Bind::Local(t, vars)) => {
                    let old = self.read_vars(&t, &vars);
                    let v = combine(self, old)?;
                    self.assign_vars(&t, &vars, &v, span)
                }
                Some(Bind::Place(p)) => {
                    let old = self.read(&p);
                    let v = combine(self, old)?;
                    self.write(&p, &v, span)
                }
                Some(Bind::Param(_)) => err(target.span, format!("`{}` is a param: set it from the Score", name)),
                Some(_) => err(target.span, format!("`{}` is a constant", name)),
                None => err(target.span, format!("unknown name `{}`", name)),
            },
            ExprKind::Field(base, field) => {
                // A lane of a vec2 local.
                if let ExprKind::Ident(name) = &base.kind {
                    if let Some(Bind::Local(T::V2, vars)) = self.lookup(name) {
                        let lane = vec2_lane(field).ok_or_else(|| {
                            ShaderError::new(target.span.start, target.span.end, format!("vec2 has x and y, not `{}`", field))
                        })?;
                        let old = V::F(self.b.get(vars[lane]));
                        let v = combine(self, old)?;
                        let x = self.to_f(&v, span)?;
                        self.b.set(vars[lane], x);
                        return Ok(());
                    }
                }
                let p = self.place(target)?;
                let old = self.read(&p);
                let v = combine(self, old)?;
                self.write(&p, &v, span)
            }
            ExprKind::Index(..) => {
                let p = self.place(target)?;
                let old = self.read(&p);
                let v = combine(self, old)?;
                self.write(&p, &v, span)
            }
            _ => err(target.span, "cannot assign to this"),
        }
    }

    /// Resolves an lvalue/aggregate expression to a memory place.
    fn place(&mut self, e: &Expr) -> LResult<Place> {
        match &e.kind {
            ExprKind::Ident(_) | ExprKind::StructLit(..) | ExprKind::ArrayRepeat(..) | ExprKind::ArrayList(..) | ExprKind::Call(..) => {
                match self.expr(e)? {
                    V::Place(p) => Ok(p),
                    v => err(e.span, format!("expected a struct or array, found {}", self.vname(&v))),
                }
            }
            ExprKind::Field(base, field) => {
                let p = self.place(base)?;
                self.field_place(&p, field, e.span)
            }
            ExprKind::Index(base, idx) => {
                let p = self.place(base)?;
                let iv = self.expr(idx)?;
                self.index_place(&p, &iv, idx.span)
            }
            _ => err(e.span, "expected a struct or array"),
        }
    }

    fn field_place(&mut self, p: &Place, field: &str, span: Span) -> LResult<Place> {
        match &p.ty {
            T::Struct(s) => {
                let d = &self.structs[*s];
                let Some((_, ft, off, _)) = d.fields.iter().find(|f| f.0 == field) else {
                    return err(span, format!("`{}` has no field `{}`", d.name, field));
                };
                Ok(Place { ty: ft.clone(), stat: p.stat + off, ..p.clone() })
            }
            T::V2 => {
                let lane = vec2_lane(field).ok_or_else(|| ShaderError::new(span.start, span.end, format!("vec2 has x and y, not `{}`", field)))?;
                Ok(Place { ty: T::F, stat: p.stat + lane as u32, ..p.clone() })
            }
            t => err(span, format!("{} has no fields", self.tname(t))),
        }
    }

    fn index_place(&mut self, p: &Place, idx: &V, span: Span) -> LResult<Place> {
        let T::Array(el, n) = &p.ty else {
            return err(span, format!("{} cannot be indexed", self.tname(&p.ty)));
        };
        let (el, n) = ((**el).clone(), *n);
        let stride = self.words(&el);
        if let V::Lit(x) = idx {
            let k = (x.floor() as i64).rem_euclid(n as i64) as u32;
            return Ok(Place { ty: el, stat: p.stat + k * stride, ..p.clone() });
        }
        let i = self.to_i(idx, span)?;
        let w = self.b.def(Ty::I32, Op::Wrap(i, n));
        let w = if stride == 1 {
            w
        } else {
            let s = self.b.ci(stride as i32);
            self.b.ib(Bin::MulI, w, s)
        };
        let off = match p.off {
            None => {
                if p.stat == 0 {
                    w
                } else {
                    let s = self.b.ci(p.stat as i32);
                    self.b.ib(Bin::AddI, w, s)
                }
            }
            Some(o) => {
                let t = if p.stat == 0 {
                    o
                } else {
                    let s = self.b.ci(p.stat as i32);
                    self.b.ib(Bin::AddI, o, s)
                };
                self.b.ib(Bin::AddI, t, w)
            }
        };
        Ok(Place { ty: el, stat: 0, off: Some(off), ..p.clone() })
    }

    fn for_loop(&mut self, var: &str, from: &Expr, to: &Expr, body: &[Stmt], span: Span) -> LResult<()> {
        let a = self.expr(from)?;
        let z = self.expr(to)?;
        if let (V::Lit(a), V::Lit(z)) = (&a, &z) {
            let (a, z) = (a.floor() as i64, z.floor() as i64);
            let count = (z - a).max(0) as u64;
            if count <= UNROLL_MAX as u64 && !has_own_break(body) {
                for k in a..z {
                    self.scoped(|l| {
                        l.bind(var, Bind::Const(V::Lit(k as f64)));
                        l.stmts(body)
                    })?;
                }
                return Ok(());
            }
            if count >= u32::MAX as u64 - 1 {
                return err(span, "loop too long");
            }
        }
        let cap = match (&a, &z) {
            (V::Lit(a), V::Lit(z)) => ((z.floor() - a.floor()).max(0.0) as u32).saturating_add(1),
            _ => LOOP_CAP,
        };
        let av = self.to_i(&a, from.span)?;
        let zv = self.to_i(&z, to.span)?;
        let iv = self.b.var(Ty::I32);
        self.b.set(iv, av);
        self.b.open();
        self.loops.push(LoopKind::User);
        let cur = self.b.get(iv);
        let done = self.b.cmpi(Cmp::Ge, cur, zv);
        self.b.push(IS::If(done, vec![IS::Break(0)], vec![]));
        let one = self.b.ci(1);
        let next = self.b.ib(Bin::AddI, cur, one);
        self.b.set(iv, next);
        let r = self.scoped(|l| {
            l.bind(var, Bind::Value(V::I(cur)));
            l.stmts(body)
        });
        self.loops.pop();
        let b = self.b.close();
        r?;
        self.b.push(IS::Loop { cap, body: b });
        Ok(())
    }

    // -- expressions -------------------------------------------------------------

    fn expr(&mut self, e: &Expr) -> LResult<V> {
        let span = e.span;
        match &e.kind {
            ExprKind::Num(x, _) => Ok(V::Lit(*x)),
            ExprKind::Bool(x) => Ok(V::B(self.b.cb(*x))),
            ExprKind::Ident(name) => self.ident(name, span),
            ExprKind::Neg(a) => {
                let v = self.expr(a)?;
                match v {
                    V::Lit(x) => Ok(V::Lit(-x)),
                    V::F(x) => Ok(V::F(self.b.fneg(x))),
                    V::I(x) => Ok(V::I(self.b.un(Ty::I32, Un::NegI, x))),
                    V::V2(x, y) => {
                        let x = self.b.fneg(x);
                        let y = self.b.fneg(y);
                        Ok(V::V2(x, y))
                    }
                    v => err(span, format!("cannot negate {}", self.vname(&v))),
                }
            }
            ExprKind::Not(a) => {
                let v = self.expr(a)?;
                let c = self.truth(&v, a.span)?;
                Ok(V::B(self.b.un(Ty::Bool, Un::NotB, c)))
            }
            ExprKind::Bin(BinOp::And, a, b) | ExprKind::Bin(BinOp::Or, a, b) => {
                let is_and = matches!(e.kind, ExprKind::Bin(BinOp::And, ..));
                let av = self.expr(a)?;
                let ac = self.truth(&av, a.span)?;
                self.b.open();
                let bv = self.expr(b);
                let bc = match bv {
                    Ok(bv) => self.truth(&bv, b.span),
                    Err(e) => Err(e),
                };
                let blk = self.b.close();
                let bc = bc?;
                if Builder::pure_block(&blk) {
                    self.b.splice(blk);
                    let op = if is_and { Bin::AndB } else { Bin::OrB };
                    return Ok(V::B(self.b.def(Ty::Bool, Op::Bin(op, ac, bc))));
                }
                let t = self.b.var(Ty::Bool);
                self.b.set(t, ac);
                let mut blk = blk;
                blk.push(IS::Set(t, bc));
                if is_and {
                    self.b.push(IS::If(ac, blk, vec![]));
                } else {
                    self.b.push(IS::If(ac, vec![], blk));
                }
                Ok(V::B(self.b.get(t)))
            }
            ExprKind::Bin(op, a, b) => {
                let av = self.expr(a)?;
                let bv = self.expr(b)?;
                self.binary(*op, av, bv, span)
            }
            ExprKind::Field(base, field) => {
                let v = self.expr(base)?;
                match v {
                    V::V2(x, y) => match vec2_lane(field) {
                        Some(0) => Ok(V::F(x)),
                        Some(_) => Ok(V::F(y)),
                        None => err(span, format!("vec2 has x and y, not `{}`", field)),
                    },
                    V::Place(p) => {
                        let fp = self.field_place(&p, field, span)?;
                        Ok(self.read(&fp))
                    }
                    v => err(span, format!("{} has no field `{}`", self.vname(&v), field)),
                }
            }
            ExprKind::Index(base, idx) => {
                let p = self.place(base)?;
                let iv = self.expr(idx)?;
                let ep = self.index_place(&p, &iv, idx.span)?;
                Ok(self.read(&ep))
            }
            ExprKind::Call(name, args) => self.call_expr(name, args, span),
            ExprKind::If(arms, else_) => self.if_expr(arms, else_.as_ref(), span),
            ExprKind::Match(subject, arms) => {
                let sv = self.expr(subject)?;
                // Lower to an if/elif chain.
                let mut chain: Vec<(Val, &Vec<crate::parse::Stmt>)> = Vec::new();
                let mut default = None;
                let mut conds = Vec::new();
                for (pats, body) in arms {
                    match pats {
                        None => {
                            default = Some(body);
                        }
                        Some(pats) => {
                            let mut c: Option<Val> = None;
                            for p in pats {
                                let pv = self.expr(p)?;
                                let eq = self.binary(BinOp::Eq, sv.clone(), pv, p.span)?;
                                let V::B(eq) = eq else { unreachable!() };
                                c = Some(match c {
                                    None => eq,
                                    Some(c) => self.b.def(Ty::Bool, Op::Bin(Bin::OrB, c, eq)),
                                });
                            }
                            conds.push((c.unwrap(), body));
                        }
                    }
                }
                chain.extend(conds);
                self.if_chain(&chain, default, span)
            }
            ExprKind::Block(stmts) => self.scoped(|l| l.stmts(stmts)),
            ExprKind::StructLit(..) | ExprKind::ArrayRepeat(..) | ExprKind::ArrayList(..) => self.aggregate_literal(e),
        }
    }

    fn aggregate_literal(&mut self, e: &Expr) -> LResult<V> {
        // Constant literals initialize from their image; otherwise field by
        // field.
        if let Ok(c) = self.cinit_quiet(e) {
            let t = self.cinit_ty(&c);
            let words = self.words(&t);
            let root = self.alloc_frame(words, e.span)?;
            let p = Place { region: Region::Frame, root, extent: words, off: None, stat: 0, ty: t };
            self.store_cinit(&p, &c);
            return Ok(V::Place(p));
        }
        match &e.kind {
            ExprKind::StructLit(name, fields) => {
                let Some(s) = self.structs.iter().position(|d| d.name == *name) else {
                    return err(e.span, format!("unknown struct `{}`", name));
                };
                let words = self.structs[s].words;
                let root = self.alloc_frame(words, e.span)?;
                let p = Place { region: Region::Frame, root, extent: words, off: None, stat: 0, ty: T::Struct(s) };
                let defaults = CInit::Struct(s, self.structs[s].fields.iter().map(|f| f.3.clone()).collect());
                self.store_cinit(&p, &defaults);
                for (fname, fe) in fields {
                    let fp = self.field_place(&p, fname, fe.span)?;
                    let v = self.expr(fe)?;
                    self.write(&fp, &v, fe.span)?;
                }
                Ok(V::Place(p))
            }
            ExprKind::ArrayList(list) => {
                let first = self.expr(&list[0])?;
                let et = match &first {
                    V::Place(p) => p.ty.clone(),
                    v => Self::value_t(v).ok_or_else(|| ShaderError::new(e.span.start, e.span.end, "bad array element".into()))?,
                };
                let ew = self.words(&et);
                let n = list.len() as u32;
                let root = self.alloc_frame(ew * n, e.span)?;
                let p = Place { region: Region::Frame, root, extent: ew * n, off: None, stat: 0, ty: T::Array(Box::new(et.clone()), n) };
                for (k, x) in list.iter().enumerate() {
                    let v = if k == 0 { first.clone() } else { self.expr(x)? };
                    let ep = Place { ty: et.clone(), stat: k as u32 * ew, ..p.clone() };
                    self.write(&ep, &v, x.span)?;
                }
                Ok(V::Place(p))
            }
            ExprKind::ArrayRepeat(el, count) => {
                let n = self.const_count(count)?;
                let v = self.expr(el)?;
                let et = match &v {
                    V::Place(p) => p.ty.clone(),
                    v => Self::value_t(v).unwrap(),
                };
                let ew = self.words(&et);
                let root = self.alloc_frame(ew * n, e.span)?;
                let p = Place { region: Region::Frame, root, extent: ew * n, off: None, stat: 0, ty: T::Array(Box::new(et.clone()), n) };
                if n > 16 {
                    return err(e.span, "a local array filled with a non-constant needs at most 16 elements; use a loop");
                }
                for k in 0..n {
                    let ep = Place { ty: et.clone(), stat: k * ew, ..p.clone() };
                    self.write(&ep, &v, e.span)?;
                }
                Ok(V::Place(p))
            }
            _ => unreachable!(),
        }
    }

    /// `cinit` without touching the main builder when it fails.
    fn cinit_quiet(&mut self, e: &Expr) -> LResult<CInit> {
        let saved_frame = self.frame_words;
        let r = self.cinit(e, None);
        self.frame_words = saved_frame;
        r
    }

    fn ident(&mut self, name: &str, span: Span) -> LResult<V> {
        match self.lookup(name) {
            Some(Bind::Const(v)) | Some(Bind::Value(v)) => return Ok(v),
            Some(Bind::Local(t, vars)) => return Ok(self.read_vars(&t, &vars)),
            Some(Bind::Place(p)) => return Ok(self.read(&p)),
            Some(Bind::Param(k)) => {
                let Some(ent) = self.entry.clone() else {
                    return err(span, format!("param `{}` is not available here", name));
                };
                let (base, step) = ent.params[k];
                let pos = self.b.get(ent.pos);
                let mask = self.b.ci(CONTROL_BLOCK as i32 - 1);
                let ph = self.b.ib(Bin::AndI, pos, mask);
                let one = self.b.ci(1);
                let ph = self.b.ib(Bin::AddI, ph, one);
                let t = self.b.un(Ty::F32, Un::I2F, ph);
                let bv = self.b.get(base);
                let sv = self.b.get(step);
                let d = self.b.mul(sv, t);
                return Ok(V::F(self.b.add(bv, d)));
            }
            None => {}
        }
        match name {
            "PI" => return Ok(V::Lit(std::f64::consts::PI)),
            "TAU" => return Ok(V::Lit(std::f64::consts::TAU)),
            "E" => return Ok(V::Lit(std::f64::consts::E)),
            _ => {}
        }
        if INPUT_NAMES.contains(&name) {
            let Some(ent) = self.entry.clone() else {
                return err(span, format!("`{}` is an audio input: not available in init() or initial values", name));
            };
            return Ok(match name {
                "sample_rate" | "SR" => V::F(self.b.get(ent.rate)),
                "note" => V::F(self.b.get(ent.note)),
                "freq" => V::F(self.b.get(ent.freq)),
                "gate" => V::F(self.b.get(ent.gate)),
                "velocity" => V::F(self.b.get(ent.velocity)),
                "trigger" => {
                    let pos = self.b.get(ent.pos);
                    let at = self.b.get(ent.trigger_at);
                    V::B(self.b.cmpi(Cmp::Eq, pos, at))
                }
                _ => unreachable!(),
            });
        }
        err(span, format!("unknown name `{}`", name))
    }

    fn if_expr(&mut self, arms: &[(Expr, Vec<crate::parse::Stmt>)], else_: Option<&Vec<crate::parse::Stmt>>, span: Span) -> LResult<V> {
        // Conditions after the first are evaluated only when reached, so
        // build nested ifs.
        let (c0, b0) = &arms[0];
        let cv = self.expr(c0)?;
        let c = self.truth(&cv, c0.span)?;
        let rest = &arms[1..];
        self.if2(c, &|l: &mut Self| l.scoped(|l| l.stmts(b0)), &|l: &mut Self| {
            if !rest.is_empty() {
                l.if_expr(rest, else_, span)
            } else if let Some(e) = else_ {
                l.scoped(|l| l.stmts(e))
            } else {
                Ok(V::Unit)
            }
        }, span)
    }

    fn if_chain(&mut self, chain: &[(Val, &Vec<crate::parse::Stmt>)], default: Option<&Vec<crate::parse::Stmt>>, span: Span) -> LResult<V> {
        if chain.is_empty() {
            return match default {
                Some(d) => self.scoped(|l| l.stmts(d)),
                None => Ok(V::Unit),
            };
        }
        let (c, body) = chain[0];
        let rest = &chain[1..];
        self.if2(c, &|l: &mut Self| l.scoped(|l| l.stmts(body)), &|l: &mut Self| l.if_chain(rest, default, span), span)
    }

    /// Two-way conditional; merges values through a select (both sides
    /// pure) or result variables.
    fn if2(
        &mut self,
        c: Val,
        then_: &dyn Fn(&mut Self) -> LResult<V>,
        else_: &dyn Fn(&mut Self) -> LResult<V>,
        span: Span,
    ) -> LResult<V> {
        if let Some(k) = self.b.cst(c) {
            return if k != 0 { then_(self) } else { else_(self) };
        }
        self.b.open();
        let tv = then_(self);
        let tb = self.b.close();
        let tv = tv?;
        self.b.open();
        let ev = else_(self);
        let eb = self.b.close();
        let ev = ev?;
        let t_diverges = matches!(tb.last(), Some(IS::Break(_)) | Some(IS::Continue(_)));
        let e_diverges = matches!(eb.last(), Some(IS::Break(_)) | Some(IS::Continue(_)));
        let tt = Self::value_t(&tv);
        let et = Self::value_t(&ev);
        let rt = match (&tt, &et) {
            (Some(a), Some(b)) => {
                if a == b || (matches!(tv, V::Lit(_)) && *b != T::B) {
                    Some(b.clone())
                } else if matches!(ev, V::Lit(_)) && *a != T::B {
                    Some(a.clone())
                } else if (*a == T::F && *b == T::I) || (*a == T::I && *b == T::F) {
                    Some(T::F)
                } else if *a == T::V2 || *b == T::V2 {
                    Some(T::V2)
                } else {
                    return err(span, format!("the branches give {} and {}", self.vname(&tv), self.vname(&ev)));
                }
            }
            (Some(a), None) if e_diverges => Some(a.clone()),
            (None, Some(b)) if t_diverges => Some(b.clone()),
            _ => None,
        };
        let Some(rt) = rt else {
            self.b.push(IS::If(c, tb, eb));
            return Ok(V::Unit);
        };
        // Both branches only compute: evaluate both, select.
        if Builder::pure_block(&tb) && Builder::pure_block(&eb) && tt.is_some() && et.is_some() {
            self.b.splice(tb);
            self.b.splice(eb);
            return self.select_v(c, &rt, &tv, &ev, span);
        }
        let vars = match &rt {
            T::V2 => vec![self.b.var(Ty::F32), self.b.var(Ty::F32)],
            t => vec![self.b.var(ty_of(t))],
        };
        // An unconditional first write keeps the variables' live ranges
        // inside this block (see arm64 liveness).
        self.zero_vars(&vars);
        let mut tb = tb;
        let mut eb = eb;
        if !t_diverges {
            self.b.open();
            self.assign_vars(&rt, &vars, &tv, span)?;
            tb.extend(self.b.close());
        }
        if !e_diverges {
            self.b.open();
            self.assign_vars(&rt, &vars, &ev, span)?;
            eb.extend(self.b.close());
        }
        self.b.push(IS::If(c, tb, eb));
        Ok(self.read_vars(&rt, &vars))
    }

    fn select_v(&mut self, c: Val, rt: &T, a: &V, b: &V, span: Span) -> LResult<V> {
        Ok(match rt {
            T::F => {
                let x = self.to_f(a, span)?;
                let y = self.to_f(b, span)?;
                V::F(self.b.sel(c, x, y))
            }
            T::I => {
                let x = self.to_i(a, span)?;
                let y = self.to_i(b, span)?;
                V::I(self.b.sel(c, x, y))
            }
            T::B => {
                let (V::B(x), V::B(y)) = (a, b) else { unreachable!() };
                V::B(self.b.sel(c, *x, *y))
            }
            T::V2 => {
                let (ax, ay) = self.to_v2(a, span)?;
                let (bx, by) = self.to_v2(b, span)?;
                V::V2(self.b.sel(c, ax, bx), self.b.sel(c, ay, by))
            }
            _ => unreachable!(),
        })
    }

    fn binary(&mut self, op: BinOp, a: V, b: V, span: Span) -> LResult<V> {
        use BinOp::*;
        // Literal folding (f64, deterministic).
        if let (V::Lit(x), V::Lit(y)) = (&a, &b) {
            let (x, y) = (*x, *y);
            let bits = |v: f64| v as i64;
            return Ok(match op {
                Add => V::Lit(x + y),
                Sub => V::Lit(x - y),
                Mul => V::Lit(x * y),
                Div => V::Lit(x / y),
                Rem => V::Lit(x - y * (x / y).trunc()),
                Lt | Le | Gt | Ge | Eq | Ne => {
                    let r = match op {
                        Lt => x < y,
                        Le => x <= y,
                        Gt => x > y,
                        Ge => x >= y,
                        Eq => x == y,
                        _ => x != y,
                    };
                    V::B(self.b.cb(r))
                }
                BitAnd => V::Lit((bits(x) & bits(y)) as f64),
                BitOr => V::Lit((bits(x) | bits(y)) as f64),
                BitXor => V::Lit((bits(x) ^ bits(y)) as f64),
                Shl => V::Lit(((bits(x) as i32).wrapping_shl(bits(y) as u32)) as f64),
                Shr => V::Lit(((bits(x) as i32).wrapping_shr(bits(y) as u32)) as f64),
                And | Or => unreachable!(),
            });
        }
        // vec2 arithmetic, lane-wise with scalar broadcast.
        if matches!(a, V::V2(..)) || matches!(b, V::V2(..)) {
            let (ax, ay) = self.to_v2(&a, span)?;
            let (bx, by) = self.to_v2(&b, span)?;
            let fop = match op {
                Add => Bin::AddF,
                Sub => Bin::SubF,
                Mul => Bin::MulF,
                Div => Bin::DivF,
                _ => return err(span, "vec2 supports + - * /"),
            };
            let x = self.b.fb(fop, ax, bx);
            let y = self.b.fb(fop, ay, by);
            return Ok(V::V2(x, y));
        }
        // Bools.
        if matches!(a, V::B(_)) || matches!(b, V::B(_)) {
            let (V::B(x), V::B(y)) = (&a, &b) else {
                return err(span, format!("cannot combine {} and {}", self.vname(&a), self.vname(&b)));
            };
            return Ok(V::B(match op {
                Eq => self.b.cmpi(Cmp::Eq, *x, *y),
                Ne => self.b.cmpi(Cmp::Ne, *x, *y),
                BitAnd => self.b.def(Ty::Bool, Op::Bin(Bin::AndB, *x, *y)),
                BitOr => self.b.def(Ty::Bool, Op::Bin(Bin::OrB, *x, *y)),
                BitXor => self.b.cmpi(Cmp::Ne, *x, *y),
                _ => return err(span, "bools support == != & | ^ && ||"),
            }));
        }
        let int = match (&a, &b) {
            (V::I(_), V::I(_)) => true,
            (V::I(_), V::Lit(y)) | (V::Lit(y), V::I(_)) => y.fract() == 0.0,
            (V::F(_) | V::Lit(_), V::F(_) | V::Lit(_)) | (V::F(_), V::I(_)) | (V::I(_), V::F(_)) => false,
            _ => return err(span, format!("cannot combine {} and {}", self.vname(&a), self.vname(&b))),
        };
        let bitop = matches!(op, BitAnd | BitOr | BitXor | Shl | Shr);
        if bitop && !int {
            return err(span, "bit operators need integers (use int(x))");
        }
        if int {
            let x = self.to_i(&a, span)?;
            let y = self.to_i(&b, span)?;
            let c = |l: &mut Self, cc| Ok(V::B(l.b.cmpi(cc, x, y)));
            return match op {
                Add => Ok(V::I(self.b.ib(Bin::AddI, x, y))),
                Sub => Ok(V::I(self.b.ib(Bin::SubI, x, y))),
                Mul => Ok(V::I(self.b.ib(Bin::MulI, x, y))),
                Div => Ok(V::I(self.b.ib(Bin::DivI, x, y))),
                Rem => Ok(V::I(self.b.ib(Bin::RemI, x, y))),
                BitAnd => Ok(V::I(self.b.ib(Bin::AndI, x, y))),
                BitOr => Ok(V::I(self.b.ib(Bin::OrI, x, y))),
                BitXor => Ok(V::I(self.b.ib(Bin::XorI, x, y))),
                Shl => Ok(V::I(self.b.ib(Bin::ShlI, x, y))),
                Shr => Ok(V::I(self.b.ib(Bin::ShrI, x, y))),
                Lt => c(self, Cmp::Lt),
                Le => c(self, Cmp::Le),
                Gt => c(self, Cmp::Gt),
                Ge => c(self, Cmp::Ge),
                Eq => c(self, Cmp::Eq),
                Ne => c(self, Cmp::Ne),
                And | Or => unreachable!(),
            };
        }
        let x = self.to_f(&a, span)?;
        let y = self.to_f(&b, span)?;
        let c = |l: &mut Self, cc| Ok(V::B(l.b.cmpf(cc, x, y)));
        match op {
            Add => Ok(V::F(self.b.add(x, y))),
            Sub => Ok(V::F(self.b.sub(x, y))),
            Mul => Ok(V::F(self.b.mul(x, y))),
            Div => Ok(V::F(self.b.div(x, y))),
            Rem => {
                // a - b * trunc(a / b)
                let q = self.b.div(x, y);
                let t = self.b.un(Ty::F32, Un::TruncF, q);
                let m = self.b.mul(y, t);
                Ok(V::F(self.b.sub(x, m)))
            }
            Lt => c(self, Cmp::Lt),
            Le => c(self, Cmp::Le),
            Gt => c(self, Cmp::Gt),
            Ge => c(self, Cmp::Ge),
            Eq => c(self, Cmp::Eq),
            Ne => c(self, Cmp::Ne),
            _ => unreachable!(),
        }
    }

    // -- builtin calls -----------------------------------------------------------

    fn call_expr(&mut self, name: &str, args: &[Expr], span: Span) -> LResult<V> {
        if let Some(f) = self.fns.get(name).cloned() {
            if matches!(name, "voice" | "effect" | "block" | "init") {
                return err(span, format!("`{}` is an entry point and cannot be called", name));
            }
            let mut vals = Vec::new();
            for a in args {
                vals.push(self.expr(a)?);
            }
            return self.call(&f, vals, span);
        }
        let mut vals = Vec::new();
        for a in args {
            vals.push(self.expr(a)?);
        }
        let n = vals.len();
        let want = |k: usize| -> LResult<()> {
            if n != k {
                err(span, format!("`{}` takes {} argument{}", name, k, if k == 1 { "" } else { "s" }))
            } else {
                Ok(())
            }
        };
        // Lane-wise unary math on f32 or vec2.
        let unary: Option<fn(&mut Builder, Val) -> Val> = match name {
            "sin" => Some(|b, x| b.sincos(x, false)),
            "cos" => Some(|b, x| b.sincos(x, true)),
            "tan" => Some(|b, x| {
                let s = b.sincos(x, false);
                let c = b.sincos(x, true);
                b.div(s, c)
            }),
            "tanh" | "fast_tanh" => Some(|b, x| b.tanh(x)),
            "exp" => Some(|b, x| b.exp(x)),
            "exp2" => Some(|b, x| {
                let y = b.mulc(x, std::f32::consts::LN_2);
                b.exp(y)
            }),
            "log" | "ln" => Some(|b, x| b.log(x)),
            "log2" => Some(|b, x| {
                let l = b.log(x);
                b.mulc(l, std::f32::consts::LOG2_E)
            }),
            "log10" => Some(|b, x| {
                let l = b.log(x);
                b.mulc(l, std::f32::consts::LOG10_E)
            }),
            "sqrt" => Some(|b, x| b.un(Ty::F32, Un::SqrtF, x)),
            "abs" => Some(|b, x| b.fabs(x)),
            "floor" => Some(|b, x| b.un(Ty::F32, Un::FloorF, x)),
            "ceil" => Some(|b, x| b.un(Ty::F32, Un::CeilF, x)),
            "round" => Some(|b, x| b.un(Ty::F32, Un::RoundF, x)),
            "trunc" => Some(|b, x| b.un(Ty::F32, Un::TruncF, x)),
            "fract" => Some(|b, x| {
                let f = b.un(Ty::F32, Un::FloorF, x);
                b.sub(x, f)
            }),
            "sign" => Some(|b, x| {
                let z = b.cf(0.0);
                let one = b.cf(1.0);
                let m1 = b.cf(-1.0);
                let pos = b.cmpf(Cmp::Gt, x, z);
                let neg = b.cmpf(Cmp::Lt, x, z);
                let s = b.sel(neg, m1, z);
                b.sel(pos, one, s)
            }),
            "midi_to_hz" | "mtof" => Some(|b, x| {
                let d = b.addc(x, -69.0);
                let y = b.mulc(d, std::f32::consts::LN_2 / 12.0);
                let e = b.exp(y);
                b.mulc(e, 440.0)
            }),
            "db_to_gain" | "db" => Some(|b, x| {
                let y = b.mulc(x, std::f32::consts::LN_10 / 20.0);
                b.exp(y)
            }),
            _ => None,
        };
        if let Some(f) = unary {
            want(1)?;
            return Ok(match &vals[0] {
                V::V2(x, y) => {
                    let x = f(&mut self.b, *x);
                    let y = f(&mut self.b, *y);
                    V::V2(x, y)
                }
                v => {
                    let x = self.to_f(v, args[0].span)?;
                    V::F(f(&mut self.b, x))
                }
            });
        }
        let f_args = |l: &mut Self, vals: &[V]| -> LResult<Vec<Val>> {
            vals.iter().zip(args).map(|(v, a)| l.to_f(v, a.span)).collect()
        };
        match name {
            "min" | "max" => {
                want(2)?;
                if let (V::I(_), V::I(_) | V::Lit(_)) | (V::Lit(_), V::I(_)) = (&vals[0], &vals[1]) {
                    let x = self.to_i(&vals[0], args[0].span)?;
                    let y = self.to_i(&vals[1], args[1].span)?;
                    let c = self.b.cmpi(if name == "min" { Cmp::Lt } else { Cmp::Gt }, x, y);
                    return Ok(V::I(self.b.sel(c, x, y)));
                }
                let op = if name == "min" { Bin::MinF } else { Bin::MaxF };
                if matches!(vals[0], V::V2(..)) || matches!(vals[1], V::V2(..)) {
                    let (ax, ay) = self.to_v2(&vals[0], span)?;
                    let (bx, by) = self.to_v2(&vals[1], span)?;
                    let x = self.b.fb(op, ax, bx);
                    let y = self.b.fb(op, ay, by);
                    return Ok(V::V2(x, y));
                }
                let v = f_args(self, &vals)?;
                Ok(V::F(self.b.fb(op, v[0], v[1])))
            }
            "clamp" => {
                want(3)?;
                if matches!(vals[0], V::V2(..)) {
                    let (x, y) = self.to_v2(&vals[0], span)?;
                    let lo = self.to_f(&vals[1], span)?;
                    let hi = self.to_f(&vals[2], span)?;
                    let x = self.b.fb(Bin::MaxF, x, lo);
                    let x = self.b.fb(Bin::MinF, x, hi);
                    let y = self.b.fb(Bin::MaxF, y, lo);
                    let y = self.b.fb(Bin::MinF, y, hi);
                    return Ok(V::V2(x, y));
                }
                if let V::I(_) = vals[0] {
                    let x = self.to_i(&vals[0], span)?;
                    let lo = self.to_i(&vals[1], span)?;
                    let hi = self.to_i(&vals[2], span)?;
                    let c = self.b.cmpi(Cmp::Lt, x, lo);
                    let x = self.b.sel(c, lo, x);
                    let c = self.b.cmpi(Cmp::Gt, x, hi);
                    return Ok(V::I(self.b.sel(c, hi, x)));
                }
                let v = f_args(self, &vals)?;
                let x = self.b.fb(Bin::MaxF, v[0], v[1]);
                Ok(V::F(self.b.fb(Bin::MinF, x, v[2])))
            }
            "mix" | "lerp" => {
                want(3)?;
                let t = self.to_f(&vals[2], args[2].span)?;
                if matches!(vals[0], V::V2(..)) || matches!(vals[1], V::V2(..)) {
                    let (ax, ay) = self.to_v2(&vals[0], span)?;
                    let (bx, by) = self.to_v2(&vals[1], span)?;
                    let dx = self.b.sub(bx, ax);
                    let dx = self.b.mul(dx, t);
                    let x = self.b.add(ax, dx);
                    let dy = self.b.sub(by, ay);
                    let dy = self.b.mul(dy, t);
                    let y = self.b.add(ay, dy);
                    return Ok(V::V2(x, y));
                }
                let v = f_args(self, &vals)?;
                let d = self.b.sub(v[1], v[0]);
                let d = self.b.mul(d, t);
                Ok(V::F(self.b.add(v[0], d)))
            }
            "pow" => {
                want(2)?;
                let v = f_args(self, &vals)?;
                Ok(V::F(self.b.pow(v[0], v[1])))
            }
            "atan2" => err(span, "atan2 is not available in audio shaders"),
            "step" => {
                want(2)?;
                let v = f_args(self, &vals)?;
                let lt = self.b.cmpf(Cmp::Lt, v[1], v[0]);
                let z = self.b.cf(0.0);
                let o = self.b.cf(1.0);
                Ok(V::F(self.b.sel(lt, z, o)))
            }
            "smoothstep" => {
                want(3)?;
                let v = f_args(self, &vals)?;
                let num = self.b.sub(v[2], v[0]);
                let den = self.b.sub(v[1], v[0]);
                let q = self.b.div(num, den);
                let z = self.b.cf(0.0);
                let o = self.b.cf(1.0);
                let t = self.b.fb(Bin::MaxF, q, z);
                let t = self.b.fb(Bin::MinF, t, o);
                let tt = self.b.mul(t, t);
                let t2 = self.b.mulc(t, 2.0);
                let three = self.b.cf(3.0);
                let inner = self.b.sub(three, t2);
                Ok(V::F(self.b.mul(tt, inner)))
            }
            "vec2" => match n {
                1 => {
                    if let V::V2(x, y) = vals[0] {
                        return Ok(V::V2(x, y));
                    }
                    let x = self.to_f(&vals[0], span)?;
                    Ok(V::V2(x, x))
                }
                2 => {
                    let v = f_args(self, &vals)?;
                    Ok(V::V2(v[0], v[1]))
                }
                _ => err(span, "vec2(x) or vec2(l, r)"),
            },
            "int" | "i32" => {
                want(1)?;
                Ok(match &vals[0] {
                    // A typed integer: `var w = int(0)` declares i32 state.
                    V::Lit(x) => V::I(self.b.ci(x.trunc() as i32)),
                    V::I(x) => V::I(*x),
                    V::F(x) => V::I(self.b.un(Ty::I32, Un::F2I, *x)),
                    V::B(x) => {
                        let one = self.b.ci(1);
                        let zero = self.b.ci(0);
                        V::I(self.b.sel(*x, one, zero))
                    }
                    v => return err(span, format!("int() of {}", self.vname(v))),
                })
            }
            "float" | "f32" => {
                want(1)?;
                Ok(match &vals[0] {
                    V::B(x) => {
                        let one = self.b.cf(1.0);
                        let zero = self.b.cf(0.0);
                        V::F(self.b.sel(*x, one, zero))
                    }
                    v => V::F(self.to_f(v, span)?),
                })
            }
            "len" => {
                want(1)?;
                match &vals[0] {
                    V::Place(Place { ty: T::Array(_, n), .. }) => Ok(V::Lit(*n as f64)),
                    v => err(span, format!("len() of {}", self.vname(v))),
                }
            }
            "read" | "read_cubic" => {
                want(2)?;
                let V::Place(p) = &vals[0] else {
                    return err(args[0].span, "read(array, position) needs an array");
                };
                if !matches!(&p.ty, T::Array(e, _) if **e == T::F) {
                    return err(args[0].span, "read() needs an array of f32");
                }
                let pos = self.to_f(&vals[1], args[1].span)?;
                let fl = self.b.un(Ty::F32, Un::FloorF, pos);
                let frac = self.b.sub(pos, fl);
                let i0 = self.b.un(Ty::I32, Un::F2I, fl);
                let tap = |l: &mut Self, k: i32| -> LResult<Val> {
                    let c = l.b.ci(k);
                    let ik = l.b.ib(Bin::AddI, i0, c);
                    let ep = l.index_place(p, &V::I(ik), span)?;
                    let V::F(x) = l.read(&ep) else { unreachable!() };
                    Ok(x)
                };
                if name == "read" {
                    let a = tap(self, 0)?;
                    let b = tap(self, 1)?;
                    let d = self.b.sub(b, a);
                    let d = self.b.mul(d, frac);
                    return Ok(V::F(self.b.add(a, d)));
                }
                // 4-point, 3rd-order Hermite.
                let xm1 = tap(self, -1)?;
                let x0 = tap(self, 0)?;
                let x1 = tap(self, 1)?;
                let x2 = tap(self, 2)?;
                // de Soras' form: c = (x1 - x-1)/2, v = x0 - x1, w = c + v,
                // a = w + v + (x2 - x0)/2, b = w + a; ((a f - b) f + c) f + x0.
                let b = &mut self.b;
                let d = b.sub(x1, xm1);
                let c = b.mulc(d, 0.5);
                let v = b.sub(x0, x1);
                let w = b.add(c, v);
                let s = b.sub(x2, x0);
                let s = b.mulc(s, 0.5);
                let a = b.add(w, v);
                let a = b.add(a, s);
                let bn = b.add(w, a);
                let t = b.mul(a, frac);
                let t = b.sub(t, bn);
                let t = b.mul(t, frac);
                let t = b.add(t, c);
                let t = b.mul(t, frac);
                Ok(V::F(b.add(t, x0)))
            }
            "rand" | "noise" => {
                want(0)?;
                let Some(ent) = self.entry.clone() else {
                    return err(span, "rand() runs per voice; not available in init() (use a fixed formula)");
                };
                let b = &mut self.b;
                let x = b.get(ent.rng);
                let c13 = b.ci(13);
                let t = b.ib(Bin::ShlI, x, c13);
                let x = b.ib(Bin::XorI, x, t);
                let c17 = b.ci(17);
                let t = b.ib(Bin::ShrUI, x, c17);
                let x = b.ib(Bin::XorI, x, t);
                let c5 = b.ci(5);
                let t = b.ib(Bin::ShlI, x, c5);
                let x = b.ib(Bin::XorI, x, t);
                b.set(ent.rng, x);
                let c9 = b.ci(9);
                let m = b.ib(Bin::ShrUI, x, c9);
                let two = b.ci(0x4000_0000);
                let m = b.ib(Bin::OrI, m, two);
                let f = b.un(Ty::F32, Un::BitsIF, m);
                Ok(V::F(b.addc(f, -3.0)))
            }
            "stop" => {
                want(0)?;
                let Some(ent) = self.entry.clone() else {
                    return err(span, "stop() is for voices");
                };
                let one = self.b.ci(1);
                self.b.set(ent.stop, one);
                Ok(V::Unit)
            }
            "param" => err(span, "param(...) is declared at the top level: `let cutoff = param(1200, 20, 20000)`"),
            _ => err(span, format!("unknown function `{}`", name)),
        }
    }
}

fn vec2_lane(field: &str) -> Option<usize> {
    match field {
        "x" | "l" | "0" => Some(0),
        "y" | "r" | "1" => Some(1),
        _ => None,
    }
}

/// Does a loop body contain a `break`/`continue` that targets this loop
/// (not one of its nested loops)?
fn has_own_break(body: &[Stmt]) -> bool {
    fn expr(e: &Expr) -> bool {
        match &e.kind {
            ExprKind::If(arms, else_) => {
                arms.iter().any(|(c, b)| expr(c) || stmts(b)) || else_.as_ref().is_some_and(|b| stmts(b))
            }
            ExprKind::Match(s, arms) => expr(s) || arms.iter().any(|(_, b)| stmts(b)),
            ExprKind::Block(b) => stmts(b),
            ExprKind::Bin(_, a, b) => expr(a) || expr(b),
            ExprKind::Neg(a) | ExprKind::Not(a) => expr(a),
            ExprKind::Call(_, args) => args.iter().any(expr),
            _ => false,
        }
    }
    fn stmts(b: &[Stmt]) -> bool {
        b.iter().any(|s| match s {
            Stmt::Break(_) | Stmt::Continue(_) => true,
            Stmt::Expr(e) => expr(e),
            Stmt::Let { value, .. } => expr(value),
            Stmt::Assign { value, .. } => expr(value),
            Stmt::Return(Some(e), _) => expr(e),
            _ => false,
        })
    }
    stmts(body)
}

const INPUT_NAMES: &[&str] = &["sample_rate", "SR", "note", "freq", "gate", "velocity", "trigger"];
const BUILTIN_NAMES: &[&str] = &[
    "sample_rate", "SR", "note", "freq", "gate", "velocity", "trigger", "PI", "TAU", "E", "voice", "effect", "block", "init",
];

/// Lowers a parsed shader into its render (and optional init) programs.
pub fn lower(items: &[Item]) -> Result<Lowered, ShaderError> {
    let mut l = Lowerer {
        b: Builder::new(),
        structs: Vec::new(),
        fns: HashMap::new(),
        globals: HashMap::new(),
        frames: vec![vec![HashMap::new()]],
        loops: Vec::new(),
        rets: Vec::new(),
        call_stack: Vec::new(),
        params: Vec::new(),
        state_init: Vec::new(),
        state_vars: Vec::new(),
        promoted: Vec::new(),
        shared_init: Vec::new(),
        frame_words: 0,
        entry: None,
        in_init: false,
    };
    l.top(items)?;
    let kind = match (l.fns.get("voice"), l.fns.get("effect")) {
        (Some(_), None) => Kind::Instrument,
        (None, Some(_)) => Kind::Effect,
        (Some(f), Some(_)) => return err(f.span, "a shader is an instrument (`fn voice()`) or an effect (`fn effect(l, r)`), not both"),
        (None, None) => {
            return Err(ShaderError::new(0, 1, "missing entry: add `fn voice() { … }` (instrument) or `fn effect(l, r) { … }` (effect)".into()))
        }
    };
    // Param smoothing words sit after the header, before user state.
    let nparams = l.params.len() as u32;
    if nparams > 0 {
        let shift = 2 * nparams;
        let mut words = l.state_init[..HEADER_WORDS as usize].to_vec();
        for p in &l.params {
            words.push(p.default.to_bits());
            words.push(0);
        }
        words.extend_from_slice(&l.state_init[HEADER_WORDS as usize..]);
        l.state_init = words;
        for sv in &mut l.state_vars {
            sv.offset += shift;
        }
        for (word, _) in &mut l.promoted {
            *word += shift;
        }
        for bind in l.globals.values_mut() {
            if let Bind::Place(p) = bind {
                if p.region == Region::State {
                    p.root += shift;
                }
            }
        }
    }
    let init = match l.fns.get("init").cloned() {
        Some(f) => Some(l.init_program(&f)?),
        None => None,
    };
    l.render(kind)?;
    let mut render = l.b.prog;
    render.body = l.b.blocks.into_iter().next().unwrap();
    render.frame_words = l.frame_words;
    dce(&mut render);
    if render.cost() > MAX_COST_PER_FRAME {
        return Err(ShaderError::new(0, 1, format!("too much work per sample (worst case {} ops); reduce loop sizes", render.cost())));
    }
    Ok(Lowered {
        kind,
        render,
        init,
        params: l.params,
        state_init: l.state_init,
        state_vars: l.state_vars,
        shared_init: l.shared_init,
    })
}
