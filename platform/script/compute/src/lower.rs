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

pub mod kernel;
mod portable;

/// Which front end a program is lowered for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    /// Audio shaders: voices and effects with state, run per sample.
    Audio,
    /// Compute kernels: stateless per-element programs over host buffers.
    Kernel,
}
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
/// break/continue of their own) are unrolled, when the unrolled code fits
/// [`UNROLL_BYTES`].
pub const UNROLL_MAX: u32 = 8;
/// Most code a fully unrolled loop may emit (estimated, every backend):
/// beyond a few hundred instructions unrolling stops paying (the loop's
/// own overhead is a few instructions an iteration) and costs instruction
/// cache, on phones' 32-64 KiB L1i above all; nested loops multiply.
pub const UNROLL_BYTES: u64 = 2048;
/// Estimated bytes of emitted code per AIR operation.
const BYTES_PER_OP: u64 = 4;
/// A kernel helper estimated at more AIR operations than this, called
/// from more than one place, is lowered once as a function and called
/// (each call costs a few instructions; its body would otherwise be copied
/// to every call site). Smaller helpers, helpers called once and calls
/// whose arguments are all constants (they fold) are inlined.
pub const INLINE_OPS: u64 = 48;
pub const MAX_STATE_WORDS: u32 = 1 << 20;
pub const MAX_SHARED_WORDS: u32 = 1 << 22;
pub const MAX_FRAME_WORDS: u32 = 1 << 14;
/// Largest constant initializer (state, shared tables, struct defaults).
pub const MAX_INIT_WORDS: u32 = 1 << 22;
/// Largest program after inlining and unrolling (AIR values).
pub const MAX_IR_VALS: usize = 1 << 20;
/// Worst-case AIR ops of one `init()` run.
pub const MAX_INIT_COST: u64 = 200_000_000;
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
    /// f64: a register value; two words (low, high) in memory.
    D,
    /// n f32 lanes: vec2, vec3, vec4, and mat4 (16, column-major).
    Vec(u8),
    Struct(usize),
    Array(Box<T>, u32),
    /// A view of host buffer region k: element type, stride and offset in
    /// words, and whether the kernel may write it.
    Buf(Box<T>, u32, u32, bool),
}

/// The lanes of a vector value (the first n are used).
type Lanes = [Val; 16];

fn lanes_of(vals: &[Val]) -> Lanes {
    let mut l = [Val(0); 16];
    l[..vals.len()].copy_from_slice(vals);
    l
}

fn vec_name(n: u8) -> &'static str {
    match n {
        2 => "vec2",
        3 => "vec3",
        4 => "vec4",
        _ => "mat4",
    }
}

/// The lane a component name selects (`x y z w`, `r g b a`; for vec2 also
/// `l r` = left, right, and `0 1`).
fn lane_of(n: u8, c: char) -> Option<usize> {
    let lane = match c {
        'x' => 0,
        'y' => 1,
        'z' => 2,
        'w' => 3,
        'l' if n == 2 => 0,
        'r' if n == 2 => 1,
        'r' => 0,
        'g' => 1,
        'b' => 2,
        'a' => 3,
        _ => return None,
    };
    (lane < n as usize && n <= 4).then_some(lane)
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
    D(Val),
    I(Val),
    B(Val),
    Vec(u8, Lanes),
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
    /// A kernel uniform (ctx word).
    Uniform(usize),
    /// A kernel emit buffer: (data region, count region, record width,
    /// slots per element).
    Emit(u8, u8, u32, u32),
}

/// A constant initializer (state, struct defaults, tables).
#[derive(Clone, Debug)]
enum CInit {
    W(u32, T),
    Vec(u8, [u32; 16]),
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

/// Value-numbering key of a pure op (loads and gets are "memory" keys:
/// dropped when a store or set may change them, and never reused across a
/// loop boundary).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Const(Ty, u64),
    Un(Un, Val),
    Bin(Bin, Val, Val),
    CmpF(Cmp, Val, Val),
    CmpI(Cmp, Val, Val),
    CmpD(Cmp, Val, Val),
    Sel(Val, Val, Val),
    Fma(ir::Fma, Val, Val, Val),
    Wrap(Val, u32),
    /// A load's type is part of it: a word stored as i32 and read as f32
    /// is not the stored value.
    Load(Region, u32, u32, Option<Val>, Ty),
    Get(Var),
    In(u8, Val),
    FrameCount,
    BufLen(u8),
}

impl Key {
    fn of(op: &Op, ty: Ty) -> Key {
        match *op {
            Op::ConstF(x) => Key::Const(ty, x.to_bits() as u64),
            Op::ConstD(x) => Key::Const(ty, x.to_bits()),
            Op::ConstI(x) => Key::Const(ty, x as u32 as u64),
            Op::ConstB(x) => Key::Const(ty, x as u64),
            Op::Get(v) => Key::Get(v),
            Op::Un(u, a) => Key::Un(u, a),
            Op::Bin(b, x, y) => Key::Bin(b, x, y),
            Op::CmpF(c, x, y) => Key::CmpF(c, x, y),
            Op::CmpI(c, x, y) => Key::CmpI(c, x, y),
            Op::CmpD(c, x, y) => Key::CmpD(c, x, y),
            Op::Sel(c, x, y) => Key::Sel(c, x, y),
            Op::Fma(k, a, b, c) => Key::Fma(k, a, b, c),
            Op::Wrap(x, n) => Key::Wrap(x, n),
            // Without an offset the extent does not matter: word `base`.
            Op::Load { region, base, extent, off } => Key::Load(region, base, if off.is_some() { extent } else { 1 }, off, ty),
            Op::In { ch, idx } => Key::In(ch, idx),
            Op::FrameCount => Key::FrameCount,
            Op::BufLen(k) => Key::BufLen(k),
        }
    }

    fn memory(&self) -> bool {
        matches!(self, Key::Load(..) | Key::Get(_))
    }
}

#[derive(Default)]
struct Builder {
    prog: Program,
    blocks: Vec<Block>,
    /// Register bits of constant values (f64 in all 64).
    consts: Vec<Option<u64>>,
    /// Per open block: values by key, and whether the block is a loop body
    /// (memory keys from outside it are not reused inside).
    cse: Vec<(HashMap<Key, Val>, bool)>,
}

impl Builder {
    fn new() -> Self {
        Builder { prog: Program::default(), blocks: vec![Vec::new()], consts: Vec::new(), cse: vec![(HashMap::new(), false)] }
    }

    fn lookup(&self, key: &Key) -> Option<Val> {
        for (map, is_loop) in self.cse.iter().rev() {
            if let Some(v) = map.get(key) {
                return Some(*v);
            }
            if *is_loop && key.memory() {
                return None;
            }
        }
        None
    }

    fn push(&mut self, s: IS) {
        match &s {
            IS::Set(var, _) => {
                let var = *var;
                for (map, _) in &mut self.cse {
                    map.remove(&Key::Get(var));
                }
            }
            IS::Store { region, base, off, val, .. } => {
                let region = *region;
                for (map, _) in &mut self.cse {
                    map.retain(|k, _| !matches!(k, Key::Load(r, ..) if *r == region));
                }
                // A later load of that word (same type) in this block reads
                // the stored value. Not host buffers: two words past a
                // buffer's end clamp to the same word.
                if off.is_none() && !matches!(region, Region::Buf(_)) {
                    let ty = self.prog.vals[val.0 as usize];
                    self.cse.last_mut().unwrap().0.insert(Key::Load(region, *base, 1, None, ty), *val);
                }
            }
            // A function may write memory (not the caller's variables, not
            // the tables).
            IS::Call { .. } => {
                for (map, _) in &mut self.cse {
                    map.retain(|k, _| !matches!(k, Key::Load(r, ..) if *r != Region::Shared));
                }
            }
            // A host call may write its slices' buffers (and ctx words).
            IS::CallHost { slices, .. } => {
                let bufs: Vec<u8> = slices.iter().map(|x| x.buf).collect();
                for (map, _) in &mut self.cse {
                    map.retain(|k, _| !matches!(k, Key::Load(Region::Buf(b), ..) if bufs.contains(b)) && !matches!(k, Key::Load(Region::Ctx, ..)));
                }
            }
            _ => {}
        }
        self.blocks.last_mut().unwrap().push(s);
    }

    fn open_loop(&mut self) {
        self.blocks.push(Vec::new());
        self.cse.push((HashMap::new(), true));
    }

    fn open(&mut self) {
        self.cse.push((HashMap::new(), false));
        self.blocks.push(Vec::new());
    }

    fn close(&mut self) -> Block {
        self.cse.pop();
        self.blocks.pop().unwrap()
    }

    fn var(&mut self, ty: Ty) -> Var {
        self.prog.vars.push(ty);
        Var(self.prog.vars.len() as u32 - 1)
    }

    /// A constant's low word (every 32-bit type).
    fn cst(&self, v: Val) -> Option<u32> {
        self.cst64(v).map(|c| c as u32)
    }

    fn cst64(&self, v: Val) -> Option<u64> {
        // A value of another builder (an outer local seen while a constant
        // initializer is folded in a scratch builder) is not a constant.
        self.consts.get(v.0 as usize).copied().flatten()
    }

    fn raw(&mut self, ty: Ty, op: Op, c: Option<u64>) -> Val {
        let key = Key::of(&op, ty);
        if let Some(v) = self.lookup(&key) {
            return v;
        }
        self.cse.last_mut().unwrap().0.insert(key, Val(self.prog.vals.len() as u32));
        let v = Val(self.prog.vals.len() as u32);
        self.prog.vals.push(ty);
        self.consts.push(c);
        self.push(IS::Def(v, op));
        v
    }

    fn cf(&mut self, x: f32) -> Val {
        self.raw(Ty::F32, Op::ConstF(x), Some(x.to_bits() as u64))
    }

    fn cd(&mut self, x: f64) -> Val {
        self.raw(Ty::F64, Op::ConstD(x), Some(x.to_bits()))
    }

    fn ci(&mut self, x: i32) -> Val {
        self.raw(Ty::I32, Op::ConstI(x), Some(x as u32 as u64))
    }

    fn cb(&mut self, x: bool) -> Val {
        self.raw(Ty::Bool, Op::ConstB(x), Some(x as u64))
    }

    fn konst(&mut self, ty: Ty, bits: u64) -> Val {
        match ty {
            Ty::F32 => self.cf(f32::from_bits(bits as u32)),
            Ty::I32 => self.ci(bits as u32 as i32),
            Ty::Bool => self.cb(bits as u32 != 0),
            Ty::F64 => self.cd(f64::from_bits(bits)),
        }
    }

    /// Emits `op` of type `ty`, folding it when every operand is constant.
    fn def(&mut self, ty: Ty, op: Op) -> Val {
        let folded = match op {
            Op::Un(u, a) => self.cst64(a).map(|a| ir::eval_un(u, a)),
            Op::Bin(b, x, y) => match (self.cst64(x), self.cst64(y)) {
                (Some(x), Some(y)) => Some(ir::eval_bin(b, x, y)),
                _ => None,
            },
            Op::CmpF(cc, x, y) => match (self.cst64(x), self.cst64(y)) {
                (Some(x), Some(y)) => Some(ir::eval_cmp_f(cc, x, y)),
                _ => None,
            },
            Op::CmpD(cc, x, y) => match (self.cst64(x), self.cst64(y)) {
                (Some(x), Some(y)) => Some(ir::eval_cmp_d(cc, x, y)),
                _ => None,
            },
            Op::CmpI(cc, x, y) => match (self.cst64(x), self.cst64(y)) {
                (Some(x), Some(y)) => Some(ir::eval_cmp_i(cc, x, y)),
                _ => None,
            },
            Op::Sel(c, x, y) => match self.cst(c) {
                Some(c) => return if c != 0 { x } else { y },
                None => None,
            },
            Op::Wrap(x, len) => self.cst(x).map(|x| ir::eval_wrap(x, len) as u64),
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
        let ty = self.prog.vals.get(x.0 as usize).copied().unwrap_or(Ty::F32);
        self.def(ty, Op::Sel(c, x, y))
    }
    fn get(&mut self, v: Var) -> Val {
        let ty = self.prog.vars.get(v.0 as usize).copied().unwrap_or(Ty::F32);
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

    /// atan: reduce |x| to [0, tan(pi/8)] (x > 2.414 -> pi/2 - atan(1/x),
    /// x > 0.414 -> pi/4 + atan((x-1)/(x+1))), then a degree-9 polynomial.
    fn atan(&mut self, x: Val) -> Val {
        let ax = self.fabs(x);
        let hi = self.cf(2.414_213_6);
        let mid = self.cf(0.414_213_57);
        let is_hi = self.cmpf(Cmp::Gt, ax, hi);
        let is_mid = self.cmpf(Cmp::Gt, ax, mid);
        let one = self.cf(1.0);
        let m1 = self.cf(-1.0);
        let inv = self.div(m1, ax);
        let num = self.sub(ax, one);
        let den = self.add(ax, one);
        let q = self.div(num, den);
        let r = self.sel(is_mid, q, ax);
        let r = self.sel(is_hi, inv, r);
        let zero = self.cf(0.0);
        let pi4 = self.cf(std::f32::consts::FRAC_PI_4);
        let pi2 = self.cf(std::f32::consts::FRAC_PI_2);
        let y0 = self.sel(is_mid, pi4, zero);
        let y0 = self.sel(is_hi, pi2, y0);
        let z = self.mul(r, r);
        let p = self.poly(z, &[8.053_744_5e-2, -1.387_768_6e-1, 1.997_771_1e-1, -3.333_295e-1]);
        let p = self.mul(p, z);
        let p = self.mul(p, r);
        let p = self.add(p, r);
        let y = self.add(y0, p);
        let xneg = self.cmpf(Cmp::Lt, x, zero);
        let ny = self.fneg(y);
        self.sel(xneg, ny, y)
    }

    /// atan2(y, x) in -pi..pi (atan2(0, 0) = 0).
    fn atan2(&mut self, y: Val, x: Val) -> Val {
        let q = self.div(y, x);
        let a = self.atan(q);
        let zero = self.cf(0.0);
        let pi = self.cf(std::f32::consts::PI);
        let npi = self.cf(-std::f32::consts::PI);
        let xneg = self.cmpf(Cmp::Lt, x, zero);
        let yneg = self.cmpf(Cmp::Lt, y, zero);
        let fix = self.sel(yneg, npi, pi);
        let a2 = self.add(a, fix);
        let a = self.sel(xneg, a2, a);
        // x == 0: +-pi/2 by the sign of y (0 when y is 0 too).
        let xz = self.cmpf(Cmp::Eq, x, zero);
        let h = self.cf(std::f32::consts::FRAC_PI_2);
        let nh = self.cf(-std::f32::consts::FRAC_PI_2);
        let yz = self.cmpf(Cmp::Eq, y, zero);
        let v = self.sel(yneg, nh, h);
        let v = self.sel(yz, zero, v);
        self.sel(xz, v, a)
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
                    IS::CallHost { args, slices, .. } => args.iter().copied().chain(slices.iter().flat_map(|x| [x.off, x.len])).collect(),
                    IS::Call { args, .. } => args.clone(),
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
    /// Spans at or past this lie in the prelude.
    prelude_base: usize,
    domain: Domain,
    /// Kernel front end: buffers, emit counters, the element value.
    kernel: kernel::KernelCtx,
    /// `math: portable`: f32 math functions evaluate in f64 with the
    /// fdlibm kernels of `makepad_csg_math::portable` and round once.
    portable: bool,
    /// Helpers lowered as functions (the program's `funcs`), each with
    /// what its calls need ([`FnMeta`]).
    funcs_out: Vec<(Program, FnMeta)>,
    /// Per helper and argument kinds (literal arguments by their bits):
    /// its function (None: inlined).
    func_cache: HashMap<(String, Vec<u8>, Vec<u64>), Option<u16>>,
    /// Call sites per helper name in the source (counted once).
    call_sites: Option<HashMap<String, usize>>,
}

/// How a call of a lowered function passes and returns values.
#[derive(Clone)]
struct FnMeta {
    /// Its params end with the element index and the emit counters'
    /// values; its results with the counters' new values.
    emits: bool,
    /// The value it returns (its leading results; None: nothing).
    ret: Option<T>,
}

fn ty_of(t: &T) -> Ty {
    match t {
        T::F => Ty::F32,
        T::I => Ty::I32,
        T::B => Ty::Bool,
        T::D => Ty::F64,
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
            T::D => 2,
            T::Vec(n) => *n as u32,
            T::Struct(s) => self.structs[*s].words,
            T::Array(e, n) => self.words(e) * n,
            T::Buf(..) => 0,
        }
    }

    fn sig(&self, t: &T) -> String {
        match t {
            T::F => "f32".into(),
            T::I => "i32".into(),
            T::B => "bool".into(),
            T::D => "f64".into(),
            T::Vec(n) => vec_name(*n).into(),
            T::Struct(s) => {
                let d = &self.structs[*s];
                let fields: Vec<String> = d.fields.iter().map(|(n, t, _, _)| format!("{}:{}", n, self.sig(t))).collect();
                format!("{}{{{}}}", d.name, fields.join(","))
            }
            T::Array(e, n) => format!("[{};{}]", self.sig(e), n),
            T::Buf(e, ..) => format!("buffer<{}>", self.sig(e)),
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
            V::D(_) => "f64".into(),
            V::Vec(n, _) => vec_name(*n).into(),
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
            // An f64 rounds once to f32.
            V::D(x) => self.b.d2f(*x),
            _ => return err(span, format!("expected a number, found {}", self.vname(v))),
        })
    }

    /// A number as f64 (f32 and i32 widen exactly; literals keep their
    /// full f64 value).
    fn to_d(&mut self, v: &V, span: Span) -> LResult<Val> {
        Ok(match v {
            V::Lit(x) => self.b.cd(*x),
            V::D(x) => *x,
            V::F(x) => self.b.f2d(*x),
            V::I(x) => self.b.un(Ty::F64, Un::I2D, *x),
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
            V::D(x) => {
                let f = self.b.un(Ty::F64, Un::FloorD, *x);
                self.b.un(Ty::I32, Un::D2I, f)
            }
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
            V::D(x) => {
                let z = self.b.cd(0.0);
                self.b.def(Ty::Bool, Op::CmpD(Cmp::Ne, *x, z))
            }
            _ => return err(span, format!("expected a condition, found {}", self.vname(v))),
        })
    }

    /// Reads a place of value type; aggregates stay places.
    fn read(&mut self, p: &Place) -> V {
        if let Region::Buf(k) = p.region {
            if !matches!(p.ty, T::Buf(..)) {
                self.kernel.access(k, p.off);
            }
        }
        let (base, extent, off) = self.addr(p);
        match p.ty {
            T::F => V::F(self.b.load(Ty::F32, p.region, base, extent, off)),
            T::I => V::I(self.b.load(Ty::I32, p.region, base, extent, off)),
            T::B => V::B(self.b.load(Ty::Bool, p.region, base, extent, off)),
            T::D => {
                let lo = self.b.load(Ty::I32, p.region, base, extent, off);
                let o = self.offset_plus(p, 1);
                let hi = self.b.load(Ty::I32, p.region, o.0, o.1, o.2);
                V::D(self.b.def(Ty::F64, Op::Bin(Bin::MakeD, hi, lo)))
            }
            T::Vec(n) => {
                let mut l = [Val(0); 16];
                l[0] = self.b.load(Ty::F32, p.region, base, extent, off);
                for k in 1..n as usize {
                    let o = self.offset_plus(p, k as u32);
                    l[k] = self.b.load(Ty::F32, p.region, o.0, o.1, o.2);
                }
                V::Vec(n, l)
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
                // The static part goes into the base (the dynamic offset of
                // an element never exceeds the object minus that part).
                let s = p.stat + k;
                if s < p.extent {
                    (p.root + s, p.extent - s, Some(o))
                } else {
                    let c = self.b.ci(s as i32);
                    let off = self.b.ib(Bin::AddI, o, c);
                    (p.root, p.extent, Some(off))
                }
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
        if let Region::Buf(k) = p.region {
            if !self.kernel.writable(k) {
                return err(span, "input buffers are read-only; declare an output(...) to write");
            }
            self.kernel.access(k, p.off);
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
            (T::D, V::D(_) | V::F(_) | V::I(_) | V::Lit(_)) => {
                let x = self.to_d(v, span)?;
                let lo = self.b.un(Ty::I32, Un::LoD, x);
                let hi = self.b.un(Ty::I32, Un::HiD, x);
                self.write_word(p, 0, lo);
                self.write_word(p, 1, hi);
            }
            (T::Vec(n), _) => {
                let n = *n;
                let l = self.to_vec(v, n, span)?;
                for k in 0..n as usize {
                    self.write_word(p, k as u32, l[k]);
                }
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
            T::F | T::Vec(_) => Ty::F32,
            T::I | T::D => Ty::I32,
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
            T::Buf(e, ..) => self.leaf_ty(e, k),
        }
    }

    fn to_v2(&mut self, v: &V, span: Span) -> LResult<(Val, Val)> {
        let l = self.to_vec(v, 2, span)?;
        Ok((l[0], l[1]))
    }

    /// A value as n lanes: a vector of that width, or a scalar broadcast.
    fn to_vec(&mut self, v: &V, n: u8, span: Span) -> LResult<Lanes> {
        match v {
            V::Vec(m, l) if *m == n => Ok(*l),
            V::Lit(_) | V::F(_) | V::I(_) | V::D(_) => {
                let x = self.to_f(v, span)?;
                Ok([x; 16])
            }
            _ => err(span, format!("expected a {}, found {}", vec_name(n), self.vname(v))),
        }
    }

    /// Swizzles and components: `v.x`, `v.zyx`, `v.xy`, `c.rgb`, `m.c2`.
    fn swizzle(&mut self, n: u8, l: &Lanes, field: &str, span: Span) -> LResult<V> {
        if n == 16 {
            if let Some(c) = field.strip_prefix('c').and_then(|c| c.parse::<usize>().ok()).filter(|c| *c < 4) {
                return Ok(V::Vec(4, lanes_of(&l[c * 4..c * 4 + 4])));
            }
            return err(span, format!("mat4 has columns c0..c3, not `{}`", field));
        }
        if let Some(k) = single_lane(n, field) {
            return Ok(V::F(l[k]));
        }
        let picked: Option<Vec<Val>> = field.chars().map(|c| lane_of(n, c).map(|k| l[k])).collect();
        match picked {
            Some(p) if (2..=4).contains(&p.len()) => Ok(V::Vec(p.len() as u8, lanes_of(&p))),
            _ => err(span, format!("{} has no component `{}` (x y z w / r g b a)", vec_name(n), field)),
        }
    }

    /// Left-associated sum of products over n lanes.
    fn dot(&mut self, a: &Lanes, b: &Lanes, n: u8) -> Val {
        let mut s = self.b.mul(a[0], b[0]);
        for k in 1..n as usize {
            let p = self.b.mul(a[k], b[k]);
            s = self.b.add(s, p);
        }
        s
    }

    /// Column-major mat4 times vec4.
    fn mat_vec(&mut self, m: &Lanes, v: &Lanes) -> V {
        let mut o = [Val(0); 16];
        for r in 0..4 {
            let mut s = self.b.mul(m[r], v[0]);
            for c in 1..4 {
                let p = self.b.mul(m[c * 4 + r], v[c]);
                s = self.b.add(s, p);
            }
            o[r] = s;
        }
        V::Vec(4, o)
    }

    /// The width of the vector among some values (None when all scalars).
    fn vec_width(&self, vals: &[&V], span: Span) -> LResult<Option<u8>> {
        let mut w = None;
        for v in vals {
            if let V::Vec(n, _) = v {
                match w {
                    None => w = Some(*n),
                    Some(m) if m == *n => {}
                    Some(m) => return err(span, format!("mixing {} and {}", vec_name(m), vec_name(*n))),
                }
            }
        }
        Ok(w)
    }

    // -- constant initializers ---------------------------------------------

    fn cinit_ty(&self, c: &CInit) -> T {
        match c {
            CInit::W(_, t) => t.clone(),
            CInit::Vec(n, _) => T::Vec(*n),
            CInit::Struct(s, _) => T::Struct(*s),
            CInit::Array(t, v) => T::Array(Box::new(t.clone()), v.len() as u32),
        }
    }

    /// Words a constant initializer flattens to.
    fn cinit_words(&self, c: &CInit) -> u32 {
        match c {
            CInit::W(..) => 1,
            CInit::Vec(n, _) => *n as u32,
            CInit::Struct(sid, _) => self.structs[*sid].words,
            CInit::Array(_, f) => f.iter().map(|c| self.cinit_words(c)).fold(0u32, |a, b| a.saturating_add(b)),
        }
    }

    /// A constant initializer's words in memory order: struct fields at
    /// their offsets, padding zero (no word of a record is left unset).
    fn flatten(&self, c: &CInit, out: &mut Vec<u32>) {
        match c {
            CInit::W(w, _) => out.push(*w),
            CInit::Vec(n, l) => out.extend_from_slice(&l[..*n as usize]),
            CInit::Struct(sid, f) => {
                let d = &self.structs[*sid];
                let start = out.len();
                out.resize(start + d.words as usize, 0);
                let mut tmp = Vec::new();
                for (c, (_, _, off, _)) in f.iter().zip(d.fields.iter()) {
                    tmp.clear();
                    self.flatten(c, &mut tmp);
                    let at = start + *off as usize;
                    let end = (at + tmp.len()).min(out.len());
                    out[at..end].copy_from_slice(&tmp[..end - at]);
                }
            }
            CInit::Array(_, f) => {
                for c in f {
                    self.flatten(c, out);
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
                // Check the size before expanding anything.
                let words = (self.cinit_words(&el) as u64).saturating_mul(n as u64);
                if words > MAX_INIT_WORDS as u64 {
                    return err(e.span, format!("this array holds {} words; the limit is {}", words, MAX_INIT_WORDS));
                }
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
                    (V::Lit(x), Some(ann @ (TypeAnn::Vec2 | TypeAnn::Vec3 | TypeAnn::Vec4))) => {
                        let b = (*x as f32).to_bits();
                        CInit::Vec(ann_width(ann), [b; 16])
                    }
                    (V::Lit(x), _) => CInit::W((*x as f32).to_bits(), T::F),
                    (V::F(_), _) => CInit::W(scratch_const(&v2, &scratch).map_or_else(bad, Ok)?, T::F),
                    (V::I(_), _) => CInit::W(scratch_const(&v2, &scratch).map_or_else(bad, Ok)?, T::I),
                    (V::B(_), _) => CInit::W(scratch_const(&v2, &scratch).map_or_else(bad, Ok)?, T::B),
                    (V::Vec(n, l), _) => {
                        let mut w = [0u32; 16];
                        for k in 0..*n as usize {
                            match scratch.cst(l[k]) {
                                Some(c) => w[k] = c,
                                None => return err(e.span, "initial values must be constants (numbers, constants, math on them)"),
                            }
                        }
                        CInit::Vec(*n, w)
                    }
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
            (CInit::W(w, T::F), T::Vec(n)) => Ok(CInit::Vec(*n, [w; 16])),
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
        self.flatten(c, &mut words);
        let t = self.cinit_ty(c);
        let n = words.len() as u32;
        let uniform = words.iter().all(|w| *w == words[0]);
        if n > 32 && uniform {
            // A fill loop instead of n stores.
            let leaf = self.leaf_ty(&t, 0);
            let j = self.b.var(Ty::I32);
            let zero = self.b.ci(0);
            self.b.set(j, zero);
            self.b.open_loop();
            let jv = self.b.get(j);
            let end = self.b.ci(n as i32);
            let done = self.b.cmpi(Cmp::Ge, jv, end);
            self.b.push(IS::If(done, vec![IS::Break(0)], vec![]));
            let x = self.b.konst(leaf, words[0] as u64);
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
            let x = self.b.konst(leaf, *w as u64);
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
                        if self.domain == Domain::Kernel && matches!(f.as_str(), "input" | "output" | "emit_buffer" | "param") {
                            let bind = self.kernel_decl(name, f, args, value.span)?;
                            self.globals.insert(name.clone(), bind);
                            continue;
                        }
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
                        let mut w = std::mem::take(&mut self.shared_init);
                        self.flatten(&c, &mut w);
                        self.shared_init = w;
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
                    if self.domain == Domain::Kernel {
                        return err(*span, "kernels have no state that persists between elements: use `let` inside the kernel, or write an output buffer");
                    }
                    self.check_new_global(name, *span)?;
                    let c = self.cinit(value, ann.as_ref())?;
                    let t = self.cinit_ty(&c);
                    let offset = self.state_init.len() as u32;
                    let mut w = std::mem::take(&mut self.state_init);
                    self.flatten(&c, &mut w);
                    self.state_init = w;
                    if self.state_init.len() as u32 > MAX_STATE_WORDS {
                        return err(*span, format!("state exceeds {} words (4 MiB) per instance", MAX_STATE_WORDS));
                    }
                    let words = self.words(&t);
                    self.state_vars.push(StateVar { name: name.clone(), sig: self.sig(&t), offset, words });
                    let bind = match t {
                        T::F | T::I | T::B => {
                            let v = self.b.var(ty_of(&t));
                            self.promoted.push((offset, v));
                            Bind::Local(t, vec![v])
                        }
                        T::Vec(n) => {
                            let vars: Vec<Var> = (0..n).map(|_| self.b.var(Ty::F32)).collect();
                            for (k, v) in vars.iter().enumerate() {
                                self.promoted.push((offset + k as u32, *v));
                            }
                            Bind::Local(T::Vec(n), vars)
                        }
                        t => Bind::Place(Place { region: Region::State, root: offset, extent: words, off: None, stat: 0, ty: t }),
                    };
                    self.globals.insert(name.clone(), bind);
                }
                Item::Fn(_) => {}
                // Resolved before lowering (kernels); audio has no modules.
                Item::Use { .. } => {}
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
        self.b.open_loop();
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
        // Nor host buffers, emit buffers or params: init() runs once at
        // compile time with none of them bound.
        self.globals.retain(|_, b| {
            !matches!(b, Bind::Local(..) | Bind::Emit(..) | Bind::Uniform(_) | Bind::Param(_))
                && !matches!(b, Bind::Place(p) if matches!(p.region, Region::State | Region::Buf(_)))
        });
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
        if prog.total_cost() > MAX_INIT_COST {
            return err(f.span, format!("init() may run {} operations; the limit is {}", prog.total_cost(), MAX_INIT_COST));
        }
        Ok(prog)
    }

    // -- calls (inlining) ------------------------------------------------------

    fn call(&mut self, f: &FnDecl, args: Vec<V>, span: Span) -> LResult<V> {
        if self.b.prog.vals.len() > MAX_IR_VALS {
            return err(span, "the program is too large after inlining (fewer or smaller helper calls, or smaller unrolled loops)");
        }
        if self.call_stack.iter().any(|n| *n == f.name) {
            return err(span, format!("`{}` calls itself; audio shaders have no recursion", f.name));
        }
        if args.len() != f.params.len() {
            return err(span, format!("`{}` takes {} arguments, got {}", f.name, f.params.len(), args.len()));
        }
        if let Some(v) = self.try_outline(f, &args, span)? {
            return Ok(v);
        }
        let mut scope = HashMap::new();
        for ((name, ann), a) in f.params.iter().zip(args) {
            let bind = match (a, ann) {
                (V::Place(p), _) => Bind::Place(p),
                (V::Lit(x), None) => Bind::Const(V::Lit(x)),
                (v, ann) => {
                    let v = match ann {
                        Some(TypeAnn::F32) => V::F(self.to_f(&v, span)?),
                        Some(TypeAnn::F64) => V::D(self.to_d(&v, span)?),
                        Some(TypeAnn::I32) => V::I(self.to_i(&v, span)?),
                        Some(ann @ (TypeAnn::Vec2 | TypeAnn::Vec3 | TypeAnn::Vec4 | TypeAnn::Mat4)) => {
                            let n = ann_width(ann);
                            V::Vec(n, self.to_vec(&v, n, span)?)
                        }
                        _ => match v {
                            V::Lit(x) => V::F(self.b.cf(x as f32)),
                            v => v,
                        },
                    };
                    // A parameter the function never assigns is its value
                    // (no copy): the element index stays recognisably the
                    // element's own.
                    if assigns(&f.body, name) {
                        self.new_local(&v, span)?
                    } else {
                        Bind::Value(v)
                    }
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
        let v = match r {
            Ok(v) => v,
            // An error inside a prelude helper is reported at the user's call.
            Err(e) if e.start >= self.prelude_base && span.start < self.prelude_base => {
                return err(span, format!("in `{}`: {}", f.name, e.message));
            }
            Err(e) => return Err(e),
        };
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

    /// Calls of helper `name` in the source (every function's body).
    fn sites(&mut self, name: &str) -> usize {
        if self.call_sites.is_none() {
            fn expr(e: &Expr, out: &mut HashMap<String, usize>) {
                match &e.kind {
                    ExprKind::Call(n, args) => {
                        *out.entry(n.clone()).or_default() += 1;
                        args.iter().for_each(|a| expr(a, out));
                    }
                    ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => expr(a, out),
                    ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => {
                        expr(a, out);
                        expr(b, out);
                    }
                    ExprKind::ArrayList(xs) => xs.iter().for_each(|x| expr(x, out)),
                    ExprKind::StructLit(_, fs) => fs.iter().for_each(|(_, x)| expr(x, out)),
                    ExprKind::Block(b) => stmts(b, out),
                    ExprKind::If(arms, else_) => {
                        for (c, b) in arms {
                            expr(c, out);
                            stmts(b, out);
                        }
                        if let Some(b) = else_ {
                            stmts(b, out);
                        }
                    }
                    ExprKind::Match(x, arms) => {
                        expr(x, out);
                        arms.iter().for_each(|(_, b)| stmts(b, out));
                    }
                    _ => {}
                }
            }
            fn stmts(b: &[Stmt], out: &mut HashMap<String, usize>) {
                for s in b {
                    match s {
                        Stmt::Let { value, .. } | Stmt::Expr(value) => expr(value, out),
                        Stmt::Assign { target, value, .. } => {
                            expr(target, out);
                            expr(value, out);
                        }
                        Stmt::Return(Some(e), _) => expr(e, out),
                        Stmt::For { from, to, body, .. } => {
                            expr(from, out);
                            expr(to, out);
                            stmts(body, out);
                        }
                        Stmt::While { cond, body, .. } => {
                            expr(cond, out);
                            stmts(body, out);
                        }
                        Stmt::Loop { body, .. } => stmts(body, out),
                        _ => {}
                    }
                }
            }
            let mut out = HashMap::new();
            for f in self.fns.values() {
                stmts(&f.body, &mut out);
            }
            self.call_sites = Some(out);
        }
        self.call_sites.as_ref().unwrap().get(name).copied().unwrap_or(0)
    }

    /// Whether `f` (or a helper it calls) emits.
    fn emits_in(&self, f: &FnDecl) -> bool {
        fn walk(l: &Lowerer, b: &[Stmt], seen: &mut Vec<String>) -> bool {
            fn expr(l: &Lowerer, e: &Expr, seen: &mut Vec<String>) -> bool {
                match &e.kind {
                    ExprKind::Call(n, args) => {
                        n == "emit"
                            || args.iter().any(|a| expr(l, a, seen))
                            || match l.fns.get(n) {
                                Some(g) if !seen.contains(n) => {
                                    seen.push(n.clone());
                                    walk(l, &g.body, seen)
                                }
                                _ => false,
                            }
                    }
                    ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => expr(l, a, seen),
                    ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => expr(l, a, seen) || expr(l, b, seen),
                    ExprKind::ArrayList(xs) => xs.iter().any(|x| expr(l, x, seen)),
                    ExprKind::StructLit(_, fs) => fs.iter().any(|(_, x)| expr(l, x, seen)),
                    ExprKind::Block(b) => walk(l, b, seen),
                    ExprKind::If(arms, else_) => arms.iter().any(|(c, b)| expr(l, c, seen) || walk(l, b, seen)) || else_.as_ref().is_some_and(|b| walk(l, b, seen)),
                    ExprKind::Match(x, arms) => expr(l, x, seen) || arms.iter().any(|(_, b)| walk(l, b, seen)),
                    _ => false,
                }
            }
            b.iter().any(|s| match s {
                Stmt::Let { value, .. } | Stmt::Expr(value) => expr(l, value, seen),
                Stmt::Assign { target, value, .. } => expr(l, target, seen) || expr(l, value, seen),
                Stmt::Return(Some(e), _) => expr(l, e, seen),
                Stmt::For { from, to, body, .. } => expr(l, from, seen) || expr(l, to, seen) || walk(l, body, seen),
                Stmt::While { cond, body, .. } => expr(l, cond, seen) || walk(l, body, seen),
                Stmt::Loop { body, .. } => walk(l, body, seen),
                _ => false,
            })
        }
        walk(self, &f.body, &mut vec![f.name.clone()])
    }

    /// A call of a helper lowered as a function (once per argument kinds),
    /// when it is big and called from several places; None: inline it.
    fn try_outline(&mut self, f: &FnDecl, args: &[V], span: Span) -> LResult<Option<V>> {
        // Only helpers of kernel element code (not the entry itself).
        if self.domain != Domain::Kernel || self.kernel.element.is_none() || self.in_init || self.call_stack.is_empty() {
            return Ok(None);
        }
        if self.globals.values().any(|b| matches!(b, Bind::Value(_) | Bind::Local(..))) {
            return Ok(None);
        }
        // Parameter annotations convert at the call, as when inlined.
        let mut vals = Vec::new();
        for ((_, ann), a) in f.params.iter().zip(args) {
            vals.push(match (ann, a) {
                (Some(TypeAnn::F32), a) => V::F(self.to_f(a, span)?),
                (Some(TypeAnn::I32), a) => V::I(self.to_i(a, span)?),
                (Some(ann @ (TypeAnn::Vec2 | TypeAnn::Vec3 | TypeAnn::Vec4 | TypeAnn::Mat4)), a) => {
                    let n = ann_width(ann);
                    V::Vec(n, self.to_vec(a, n, span)?)
                }
                (Some(_), _) => return Ok(None),
                (None, a) => a.clone(),
            });
        }
        // A literal argument stays a constant in the function (it folds
        // there as when inlined): one function per literal values.
        let mut kinds = Vec::new();
        let mut lits = Vec::new();
        for v in &vals {
            kinds.push(match v {
                V::Lit(x) => {
                    lits.push(x.to_bits());
                    3u8
                }
                V::F(_) => 0u8,
                V::I(_) => 1,
                V::B(_) => 2,
                V::Vec(n, _) => 10 + *n,
                _ => return Ok(None),
            });
        }
        if !vals.is_empty() && vals.iter().all(|v| matches!(v, V::Lit(_))) {
            return Ok(None);
        }
        let key = (f.name.clone(), kinds.clone(), lits);
        let idx = match self.func_cache.get(&key) {
            Some(x) => *x,
            None => {
                let big = self.sites(&f.name) >= 2 && self.ast_cost(&f.body) > INLINE_OPS;
                let x = if big { self.lower_function(f, &vals, span)? } else { None };
                self.func_cache.insert(key, x);
                x
            }
        };
        let Some(idx) = idx else { return Ok(None) };
        let meta = self.funcs_out[idx as usize].1.clone();
        // The arguments (constants as f32 values), the emit context.
        let mut argv = Vec::new();
        for v in &vals {
            match v {
                V::Vec(n, l) => argv.extend_from_slice(&l[..*n as usize]),
                V::B(x) | V::I(x) | V::F(x) => argv.push(*x),
                // Bound in the function itself.
                _ => {}
            }
        }
        let counters: Vec<Var> = self.kernel.counters.iter().map(|(_, c)| *c).collect();
        if meta.emits {
            argv.push(self.kernel.element.unwrap());
            for c in &counters {
                let x = self.b.get(*c);
                argv.push(x);
            }
        }
        let tys: Vec<Ty> = {
            let g = &self.funcs_out[idx as usize].0;
            g.results.iter().map(|r| g.vals[r.0 as usize]).collect()
        };
        let rets: Vec<Val> = tys
            .iter()
            .map(|t| {
                self.b.prog.vals.push(*t);
                self.b.consts.push(None);
                Val(self.b.prog.vals.len() as u32 - 1)
            })
            .collect();
        self.b.push(IS::Call { f: idx, args: argv, rets: rets.clone() });
        let words = match &meta.ret {
            Some(T::Vec(n)) => *n as usize,
            Some(_) => 1,
            None => 0,
        };
        if meta.emits {
            for (c, r) in counters.iter().zip(&rets[words..]) {
                self.b.set(*c, *r);
            }
        }
        Ok(Some(match meta.ret {
            Some(T::F) => V::F(rets[0]),
            Some(T::I) => V::I(rets[0]),
            Some(T::B) => V::B(rets[0]),
            Some(T::Vec(n)) => {
                let mut l = [rets[0]; 16];
                l[..n as usize].copy_from_slice(&rets[..n as usize]);
                V::Vec(n, l)
            }
            _ => V::Unit,
        }))
    }

    /// Lowers helper `f` with arguments of `kinds` as a function of the
    /// program; None when it cannot be one (it returns an aggregate, or its
    /// writes would no longer be provably the element's own): then it is
    /// inlined.
    fn lower_function(&mut self, f: &FnDecl, args: &[V], span: Span) -> LResult<Option<u16>> {
        let emits = self.emits_in(f);
        let saved_b = std::mem::replace(&mut self.b, Builder::new());
        let saved_elem = self.kernel.element;
        let saved_counters = self.kernel.counters.clone();
        let saved_offsets = std::mem::take(&mut self.kernel.local_offsets);
        let saved_nonlocal = self.kernel.nonlocal;
        let saved_loops = std::mem::take(&mut self.loops);
        let r = self.function_body(f, args, emits, span);
        let fb = std::mem::replace(&mut self.b, saved_b);
        let nonlocal = self.kernel.nonlocal && !saved_nonlocal;
        self.kernel.element = saved_elem;
        self.kernel.counters = saved_counters;
        self.kernel.local_offsets = saved_offsets;
        self.kernel.nonlocal = saved_nonlocal;
        self.loops = saved_loops;
        let Some((params, results, ret)) = r? else { return Ok(None) };
        if nonlocal {
            return Ok(None);
        }
        let mut g = fb.prog;
        g.body = fb.blocks.into_iter().next().unwrap();
        g.params = params;
        g.results = results;
        self.funcs_out.push((g, FnMeta { emits, ret }));
        Ok(Some(self.funcs_out.len() as u16 - 1))
    }

    /// The body of function `f` in the (fresh) builder: (params, results,
    /// returned type), or None when it returns what a function cannot.
    #[allow(clippy::type_complexity)]
    fn function_body(&mut self, f: &FnDecl, args: &[V], emits: bool, span: Span) -> LResult<Option<(Vec<Val>, Vec<Val>, Option<T>)>> {
        let mut params = Vec::new();
        let mut param = |l: &mut Lowerer, t: Ty| {
            l.b.prog.vals.push(t);
            l.b.consts.push(None);
            let v = Val(l.b.prog.vals.len() as u32 - 1);
            params.push(v);
            v
        };
        let mut scope = HashMap::new();
        for ((name, _), a) in f.params.iter().zip(args) {
            let v = match a {
                V::Lit(x) => {
                    // As when inlined: a constant.
                    scope.insert(name.clone(), Bind::Const(V::Lit(*x)));
                    continue;
                }
                V::F(_) => V::F(param(self, Ty::F32)),
                V::I(_) => V::I(param(self, Ty::I32)),
                V::B(_) => V::B(param(self, Ty::Bool)),
                V::Vec(n, _) => {
                    let mut l = [Val(0); 16];
                    for x in l.iter_mut().take(*n as usize) {
                        *x = param(self, Ty::F32);
                    }
                    V::Vec(*n, l)
                }
                _ => unreachable!("kinds checked"),
            };
            let bind = if assigns(&f.body, name) { self.new_local(&v, span)? } else { Bind::Value(v) };
            scope.insert(name.clone(), bind);
        }
        // The emit context: the element index, the counters (as the
        // function's own variables, their values in and out).
        let mut counters = Vec::new();
        if emits {
            let e = param(self, Ty::I32);
            self.kernel.element = Some(e);
            let kds: Vec<u8> = self.kernel.counters.iter().map(|(k, _)| *k).collect();
            self.kernel.counters.clear();
            for kd in kds {
                let c = self.b.var(Ty::I32);
                let x = param(self, Ty::I32);
                self.b.set(c, x);
                self.kernel.counters.push((kd, c));
                counters.push(c);
            }
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
        let v = match r {
            Ok(v) => v,
            Err(e) if e.start >= self.prelude_base && span.start < self.prelude_base => {
                return err(span, format!("in `{}`: {}", f.name, e.message));
            }
            Err(e) => return Err(e),
        };
        // The returned value, as when inlined.
        let v = if !ret.used {
            self.b.splice(body);
            v
        } else {
            let (t, vars) = ret.vars.clone().unwrap_or((T::F, vec![]));
            let mut body = body;
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
                V::Unit
            } else {
                self.read_vars(&t, &vars)
            }
        };
        let (t, words): (Option<T>, Vec<Val>) = match v {
            V::Unit => (None, vec![]),
            V::Lit(_) | V::F(_) => (Some(T::F), vec![self.to_f(&v, span)?]),
            V::I(x) => (Some(T::I), vec![x]),
            V::B(x) => (Some(T::B), vec![x]),
            V::Vec(n, l) => (Some(T::Vec(n)), l[..n as usize].to_vec()),
            V::D(_) | V::Place(_) => return Ok(None),
        };
        // Results are read at the end of the top level (through variables:
        // a value of a nested block is not visible there).
        let mut results = Vec::new();
        for w in words {
            let ty = self.b.prog.vals[w.0 as usize];
            let c = self.b.var(ty);
            self.b.set(c, w);
            results.push(c);
        }
        let mut results: Vec<Val> = results.into_iter().map(|c| self.b.get(c)).collect();
        for c in counters {
            results.push(self.b.get(c));
        }
        Ok(Some((params, results, t)))
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
            V::D(_) => (T::D, vec![self.b.var(Ty::F64)]),
            V::Vec(n, _) => (T::Vec(*n), (0..*n).map(|_| self.b.var(Ty::F32)).collect()),
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
            T::D => {
                let x = self.to_d(v, span)?;
                self.b.set(vars[0], x);
            }
            T::Vec(n) => {
                let l = self.to_vec(v, *n, span)?;
                for k in 0..*n as usize {
                    self.b.set(vars[k], l[k]);
                }
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
            T::D => V::D(self.b.get(vars[0])),
            T::Vec(n) => {
                let mut l = [Val(0); 16];
                for k in 0..*n as usize {
                    l[k] = self.b.get(vars[k]);
                }
                V::Vec(*n, l)
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
            V::D(_) => T::D,
            V::Vec(n, _) => T::Vec(*n),
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
                        Some(TypeAnn::F64) => V::D(self.to_d(&v, *span)?),
                        Some(ann @ (TypeAnn::Vec2 | TypeAnn::Vec3 | TypeAnn::Vec4 | TypeAnn::Mat4)) => {
                            let n = ann_width(ann);
                            V::Vec(n, self.to_vec(&v, n, *span)?)
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
                    self.b.open_loop();
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
                    self.capped_loop(b, true);
                }
                Stmt::Loop { body, .. } => {
                    self.b.open_loop();
                    self.loops.push(LoopKind::User);
                    let r = self.scoped(|l| l.stmts(body));
                    self.loops.pop();
                    let b = self.b.close();
                    r?;
                    self.capped_loop(b, false);
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
                                return err(*span, "functions return f32, i32, bool, vec2/3/4 or mat4 (structs and arrays are passed by reference instead)");
                            };
                            let vars = match &t {
                                T::Vec(n) => (0..*n).map(|_| self.b.var(Ty::F32)).collect(),
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
                // A lane of a vector local.
                if let ExprKind::Ident(name) = &base.kind {
                    if let Some(Bind::Local(T::Vec(n), vars)) = self.lookup(name) {
                        let lane = single_lane(n, field).ok_or_else(|| {
                            ShaderError::new(target.span.start, target.span.end, format!("{} has no single component `{}`", vec_name(n), field))
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
                    let fields: Vec<&str> = d.fields.iter().map(|f| f.0.as_str()).collect();
                    return err(span, format!("`{}` has no field `{}` (it has {})", d.name, field, fields.join(", ")));
                };
                Ok(Place { ty: ft.clone(), stat: p.stat + off, ..p.clone() })
            }
            T::Vec(n) => {
                let n = *n;
                if n == 16 {
                    if let Some(c) = field.strip_prefix('c').and_then(|c| c.parse::<u32>().ok()).filter(|c| *c < 4) {
                        return Ok(Place { ty: T::Vec(4), stat: p.stat + c * 4, ..p.clone() });
                    }
                }
                let lane = single_lane(n, field).ok_or_else(|| ShaderError::new(span.start, span.end, format!("{} has no single component `{}`", vec_name(n), field)))?;
                Ok(Place { ty: T::F, stat: p.stat + lane as u32, ..p.clone() })
            }
            t => err(span, format!("{} has no fields", self.tname(t))),
        }
    }

    fn index_place(&mut self, p: &Place, idx: &V, span: Span) -> LResult<Place> {
        if let T::Buf(el, stride, offset, _) = &p.ty {
            // Host buffers clamp (no wrap): element idx at idx * stride.
            let (el, stride, offset) = ((**el).clone(), *stride, *offset);
            let i = self.to_i(idx, span)?;
            let off = if stride == 1 {
                i
            } else {
                let c = self.b.ci(stride as i32);
                self.b.ib(Bin::MulI, i, c)
            };
            if self.kernel.element == Some(i) {
                self.kernel.local_offsets.insert(off);
            }
            return Ok(Place { ty: el, stat: p.stat + offset, off: Some(off), ..p.clone() });
        }
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
        // The static part (field offsets) stays static; it folds into the
        // access's base.
        let off = match p.off {
            None => w,
            Some(o) => self.b.ib(Bin::AddI, o, w),
        };
        Ok(Place { ty: el, off: Some(off), ..p.clone() })
    }

    /// Whether loops that reach their run-time cap set the call's overflow
    /// word: a kernel's element code (audio and init() keep plain caps).
    fn reports_caps(&self) -> bool {
        self.domain == Domain::Kernel && self.kernel.element.is_some()
    }

    /// A `while` or `loop` body under the run-time cap. In a kernel, a loop
    /// that starts a 1025th iteration sets the overflow word and stops
    /// (reported, never silently cut). A `while` is checked after its
    /// condition (`cond_first`: the body's first statement is the
    /// condition's break), so one that ends after exactly 1024 is not.
    fn capped_loop(&mut self, mut body: Block, cond_first: bool) {
        if !self.reports_caps() {
            self.b.push(IS::Loop { cap: LOOP_CAP, body });
            return;
        }
        let n = self.b.var(Ty::I32);
        let zero = self.b.ci(0);
        self.b.set(n, zero);
        self.b.open();
        let k = self.b.get(n);
        let most = self.b.ci(LOOP_CAP as i32);
        let hit = self.b.cmpi(Cmp::Ge, k, most);
        let one = self.b.ci(1);
        self.b.push(IS::If(hit, vec![IS::Store { region: Region::Ctx, base: crate::lower::kernel::K_OVERFLOW, extent: 1, off: None, val: one }, IS::Break(0)], vec![]));
        let one = self.b.ci(1);
        let k1 = self.b.ib(Bin::AddI, k, one);
        self.b.set(n, k1);
        let check = self.b.close();
        let mut outer = Vec::with_capacity(body.len() + check.len());
        if cond_first && !body.is_empty() {
            // The condition's value definitions and its break come first.
            let at = body.iter().position(|s| matches!(s, IS::If(..))).map_or(0, |k| k + 1);
            outer.extend(body.drain(..at));
        }
        outer.extend(check);
        outer.extend(body);
        self.b.push(IS::Loop { cap: LOOP_CAP + 1, body: outer });
    }

    /// A rough count of the AIR operations `body` lowers to (helpers
    /// inlined, constant loops at their trip count, a builtin call a few
    /// ops): what unrolling it costs.
    fn ast_cost(&self, body: &[Stmt]) -> u64 {
        fn trip(l: &Lowerer, e: &Expr) -> Option<f64> {
            match &e.kind {
                ExprKind::Num(x, _) => Some(*x),
                ExprKind::Ident(n) => match l.globals.get(n) {
                    Some(Bind::Const(V::Lit(x))) => Some(*x),
                    _ => None,
                },
                _ => None,
            }
        }
        fn expr(l: &Lowerer, e: &Expr, memo: &mut HashMap<String, u64>, stack: &mut Vec<String>) -> u64 {
            1 + match &e.kind {
                ExprKind::Num(..) | ExprKind::Bool(_) | ExprKind::Ident(_) => 0,
                ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => expr(l, a, memo, stack),
                ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => expr(l, a, memo, stack) + expr(l, b, memo, stack),
                ExprKind::ArrayList(xs) => xs.iter().map(|x| expr(l, x, memo, stack)).sum(),
                ExprKind::StructLit(_, fs) => fs.iter().map(|(_, x)| expr(l, x, memo, stack)).sum(),
                ExprKind::Block(b) => stmts(l, b, memo, stack),
                ExprKind::If(arms, else_) => arms.iter().map(|(c, b)| expr(l, c, memo, stack) + stmts(l, b, memo, stack)).sum::<u64>() + else_.as_ref().map_or(0, |b| stmts(l, b, memo, stack)),
                ExprKind::Match(x, arms) => expr(l, x, memo, stack) + arms.iter().map(|(_, b)| 2 + stmts(l, b, memo, stack)).sum::<u64>(),
                ExprKind::Call(name, args) => {
                    let a: u64 = args.iter().map(|x| expr(l, x, memo, stack)).sum();
                    a + match l.fns.get(name) {
                        Some(f) if !stack.contains(name) => match memo.get(name) {
                            Some(c) => *c,
                            None => {
                                stack.push(name.clone());
                                let c = stmts(l, &f.body, memo, stack);
                                stack.pop();
                                memo.insert(name.clone(), c);
                                c
                            }
                        },
                        _ if name == "emit" => 3 * args.len() as u64 + 8,
                        _ => 6,
                    }
                }
            }
        }
        fn stmts(l: &Lowerer, b: &[Stmt], memo: &mut HashMap<String, u64>, stack: &mut Vec<String>) -> u64 {
            b.iter()
                .map(|s| match s {
                    Stmt::Let { value, .. } | Stmt::Expr(value) => expr(l, value, memo, stack),
                    Stmt::Assign { target, value, .. } => expr(l, target, memo, stack) + expr(l, value, memo, stack),
                    Stmt::Return(e, _) => e.as_ref().map_or(1, |e| expr(l, e, memo, stack)),
                    Stmt::For { from, to, body, .. } => {
                        let n = match (trip(l, from), trip(l, to)) {
                            (Some(a), Some(z)) => (z.floor() - a.floor()).clamp(1.0, 1e6) as u64,
                            _ => 1,
                        };
                        let inner = stmts(l, body, memo, stack);
                        // A loop it would not unroll costs its body once.
                        if n <= UNROLL_MAX as u64 && n.saturating_mul(inner).saturating_mul(BYTES_PER_OP) <= UNROLL_BYTES {
                            n * inner
                        } else {
                            inner + 6
                        }
                    }
                    Stmt::While { cond, body, .. } => expr(l, cond, memo, stack) + stmts(l, body, memo, stack) + 6,
                    Stmt::Loop { body, .. } => stmts(l, body, memo, stack) + 6,
                    Stmt::Break(_) | Stmt::Continue(_) => 1,
                })
                .sum()
        }
        stmts(self, body, &mut HashMap::new(), &mut Vec::new())
    }

    fn for_loop(&mut self, var: &str, from: &Expr, to: &Expr, body: &[Stmt], span: Span) -> LResult<()> {
        let a = self.expr(from)?;
        let z = self.expr(to)?;
        if let (V::Lit(a), V::Lit(z)) = (&a, &z) {
            let (a, z) = (a.floor() as i64, z.floor() as i64);
            let count = (z - a).max(0) as u64;
            // Small loops unroll while the unrolled code stays small.
            let unroll = count <= UNROLL_MAX as u64 && count.saturating_mul(self.ast_cost(body)).saturating_mul(BYTES_PER_OP) <= UNROLL_BYTES;
            if unroll && !has_own_break(body) {
                for k in a..z {
                    self.scoped(|l| {
                        l.bind(var, Bind::Const(V::Lit(k as f64)));
                        l.stmts(body)
                    })?;
                    if self.b.prog.vals.len() > MAX_IR_VALS {
                        return err(span, "the program is too large after unrolling");
                    }
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
        if cap == LOOP_CAP && self.reports_caps() {
            // A run-time range longer than the cap is reported, not
            // silently cut: the call's overflow word.
            let len = self.b.ib(Bin::SubI, zv, av);
            let most = self.b.ci(LOOP_CAP as i32);
            let over = self.b.cmpi(Cmp::Gt, len, most);
            let one = self.b.ci(1);
            self.b.push(IS::If(over, vec![IS::Store { region: Region::Ctx, base: crate::lower::kernel::K_OVERFLOW, extent: 1, off: None, val: one }], vec![]));
        }
        let iv = self.b.var(Ty::I32);
        self.b.set(iv, av);
        self.b.open_loop();
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
                    V::D(x) => Ok(V::D(self.b.un(Ty::F64, Un::NegD, x))),
                    V::I(x) => Ok(V::I(self.b.un(Ty::I32, Un::NegI, x))),
                    V::Vec(n, l) => {
                        let mut o = l;
                        for k in 0..n as usize {
                            o[k] = self.b.fneg(l[k]);
                        }
                        Ok(V::Vec(n, o))
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
                    V::Vec(n, l) => self.swizzle(n, &l, field, span),
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
            Some(Bind::Uniform(k)) => return Ok(V::F(self.b.load(Ty::F32, Region::Ctx, kernel::K_PARAMS + k as u32, 1, None))),
            Some(Bind::Emit(..)) => return err(span, format!("`{}` is an emit buffer: write it with emit({}, ...)", name, name)),
            None => {}
        }
        if self.domain == Domain::Kernel {
            if let Some(v) = self.kernel_ident(name) {
                return Ok(v);
            }
        }
        match name {
            "PI" => return Ok(V::Lit(std::f64::consts::PI)),
            "TAU" => return Ok(V::Lit(std::f64::consts::TAU)),
            "E" => return Ok(V::Lit(std::f64::consts::E)),
            _ => {}
        }
        if INPUT_NAMES.contains(&name) && self.domain == Domain::Audio {
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
        let hint = self.suggest(name, false);
        err(span, format!("unknown name `{}`{}", name, hint))
    }

    /// ", did you mean `x`?" for a near miss among the names in scope (or
    /// the functions, for a call).
    fn suggest(&self, name: &str, call: bool) -> String {
        let mut cands: Vec<String> = Vec::new();
        if call {
            cands.extend(self.fns.keys().cloned());
            cands.extend(BUILTIN_FNS.iter().map(|s| s.to_string()));
        } else {
            if let Some(frame) = self.frames.last() {
                for scope in frame {
                    cands.extend(scope.keys().cloned());
                }
            }
            cands.extend(self.globals.keys().cloned());
            cands.extend(INPUT_NAMES.iter().map(|s| s.to_string()));
            cands.extend(["PI", "TAU", "E"].iter().map(|s| s.to_string()));
        }
        let best = cands
            .iter()
            .filter(|c| !c.contains("fuse_"))
            .map(|c| (edit_distance(name, c), c))
            .filter(|(d, c)| *d <= 2.max(name.len() / 4) && *d < c.len())
            .min();
        match best {
            Some((_, c)) => format!(", did you mean `{}`?", c),
            None => String::new(),
        }
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
                } else if (*a == T::D && matches!(b, T::F | T::I)) || (*b == T::D && matches!(a, T::F | T::I)) {
                    Some(T::D)
                } else if matches!((a, b), (T::Vec(_), T::F | T::I) | (T::F | T::I, T::Vec(_))) {
                    Some(if let T::Vec(n) = a { T::Vec(*n) } else { b.clone() })
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
        // Small branches that only compute: evaluate both and select (no
        // branch). Big ones branch, so only the taken side costs.
        if Builder::pure_block(&tb) && Builder::pure_block(&eb) && tb.len() + eb.len() <= 24 && tt.is_some() && et.is_some() {
            self.b.splice(tb);
            self.b.splice(eb);
            return self.select_v(c, &rt, &tv, &ev, span);
        }
        let vars = match &rt {
            T::Vec(n) => (0..*n).map(|_| self.b.var(Ty::F32)).collect(),
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
            T::D => {
                let x = self.to_d(a, span)?;
                let y = self.to_d(b, span)?;
                V::D(self.b.sel(c, x, y))
            }
            T::Vec(n) => {
                let la = self.to_vec(a, *n, span)?;
                let lb = self.to_vec(b, *n, span)?;
                let mut o = la;
                for k in 0..*n as usize {
                    o[k] = self.b.sel(c, la[k], lb[k]);
                }
                V::Vec(*n, o)
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
                ShrU => V::Lit(((bits(x) as i32 as u32).wrapping_shr(bits(y) as u32)) as i32 as f64),
                And | Or => unreachable!(),
            });
        }
        // Matrix products.
        if let (V::Vec(16, m), Mul) = (&a, op) {
            match &b {
                V::Vec(4, v) => return Ok(self.mat_vec(m, v)),
                V::Vec(16, m2) => {
                    let mut o = [Val(0); 16];
                    for c in 0..4 {
                        let col = lanes_of(&m2[c * 4..c * 4 + 4]);
                        let V::Vec(_, r) = self.mat_vec(m, &col) else { unreachable!() };
                        o[c * 4..c * 4 + 4].copy_from_slice(&r[..4]);
                    }
                    return Ok(V::Vec(16, o));
                }
                _ => {}
            }
        }
        // Vector arithmetic, lane-wise with scalar broadcast.
        if matches!(a, V::Vec(..)) || matches!(b, V::Vec(..)) {
            let n = self.vec_width(&[&a, &b], span)?.unwrap();
            let la = self.to_vec(&a, n, span)?;
            let lb = self.to_vec(&b, n, span)?;
            let fop = match op {
                Add => Bin::AddF,
                Sub => Bin::SubF,
                Mul => Bin::MulF,
                Div => Bin::DivF,
                _ => return err(span, format!("{} supports + - * / (compare components one by one)", vec_name(n))),
            };
            let mut o = la;
            for k in 0..n as usize {
                o[k] = self.b.fb(fop, la[k], lb[k]);
            }
            return Ok(V::Vec(n, o));
        }
        // f64: the other side widens exactly.
        if matches!(a, V::D(_)) || matches!(b, V::D(_)) {
            let x = self.to_d(&a, span)?;
            let y = self.to_d(&b, span)?;
            let arith = |l: &mut Self, op: Bin| Ok(V::D(l.b.def(Ty::F64, Op::Bin(op, x, y))));
            let c = |l: &mut Self, cc| Ok(V::B(l.b.def(Ty::Bool, Op::CmpD(cc, x, y))));
            return match op {
                Add => arith(self, Bin::AddD),
                Sub => arith(self, Bin::SubD),
                Mul => arith(self, Bin::MulD),
                Div => arith(self, Bin::DivD),
                Rem => {
                    // a - b * trunc(a / b)
                    let q = self.b.def(Ty::F64, Op::Bin(Bin::DivD, x, y));
                    let t = self.b.un(Ty::F64, Un::TruncD, q);
                    let m = self.b.def(Ty::F64, Op::Bin(Bin::MulD, y, t));
                    Ok(V::D(self.b.def(Ty::F64, Op::Bin(Bin::SubD, x, m))))
                }
                Lt => c(self, Cmp::Lt),
                Le => c(self, Cmp::Le),
                Gt => c(self, Cmp::Gt),
                Ge => c(self, Cmp::Ge),
                Eq => c(self, Cmp::Eq),
                Ne => c(self, Cmp::Ne),
                _ => err(span, "f64 supports + - * / % and comparisons"),
            };
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
        let bitop = matches!(op, BitAnd | BitOr | BitXor | Shl | Shr | ShrU);
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
                ShrU => Ok(V::I(self.b.ib(Bin::ShrUI, x, y))),
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

    /// f64 arithmetic and portable math: `f64(x)`; math on f64 arguments
    /// (always the portable kernels, giving f64); f32 math in
    /// `math: portable` (in f64, rounded once); and asin, acos, hypot and
    /// cbrt, which only exist in the portable form.
    fn f64_call(&mut self, name: &str, vals: &[V], args: &[Expr], span: Span) -> LResult<Option<V>> {
        type D1 = fn(&mut Builder, Val) -> Val;
        type D2 = fn(&mut Builder, Val, Val) -> Val;
        let un: Option<D1> = match name {
            "sin" => Some(|b, x| b.p_sin(x)),
            "cos" => Some(|b, x| b.p_cos(x)),
            "tan" => Some(|b, x| b.p_tan(x)),
            "atan" => Some(|b, x| b.p_atan(x)),
            "asin" => Some(|b, x| b.p_asin_acos(x, false)),
            "acos" => Some(|b, x| b.p_asin_acos(x, true)),
            "exp" => Some(|b, x| b.p_exp(x)),
            "log" | "ln" => Some(|b, x| b.p_ln(x)),
            "cbrt" => Some(|b, x| b.p_cbrt(x)),
            "sqrt" => Some(|b, x| b.un(Ty::F64, Un::SqrtD, x)),
            "abs" => Some(|b, x| b.un(Ty::F64, Un::AbsD, x)),
            "floor" => Some(|b, x| b.un(Ty::F64, Un::FloorD, x)),
            "ceil" => Some(|b, x| b.un(Ty::F64, Un::CeilD, x)),
            "round" => Some(|b, x| b.un(Ty::F64, Un::RoundD, x)),
            "trunc" => Some(|b, x| b.un(Ty::F64, Un::TruncD, x)),
            "fract" => Some(|b, x| {
                let f = b.un(Ty::F64, Un::FloorD, x);
                b.def(Ty::F64, Op::Bin(Bin::SubD, x, f))
            }),
            _ => None,
        };
        let bin: Option<D2> = match name {
            "pow" => Some(|b, x, y| b.p_pow(x, y)),
            "atan2" => Some(|b, y, x| b.p_atan2(y, x)),
            "hypot" => Some(|b, x, y| b.p_hypot(x, y)),
            "min" => Some(|b, x, y| b.def(Ty::F64, Op::Bin(Bin::MinD, x, y))),
            "max" => Some(|b, x, y| b.def(Ty::F64, Op::Bin(Bin::MaxD, x, y))),
            _ => None,
        };
        let any_d = vals.iter().any(|v| matches!(v, V::D(_)));
        // Functions that are only portable, and the f32 math that
        // `math: portable` routes through f64.
        let only_portable = matches!(name, "asin" | "acos" | "hypot" | "cbrt");
        let exact = matches!(name, "sqrt" | "abs" | "floor" | "ceil" | "round" | "trunc" | "fract" | "min" | "max");
        let portable_f32 = (self.portable || only_portable) && !exact;
        if name == "f64" {
            if vals.len() != 1 {
                return err(span, "`f64` takes 1 argument");
            }
            return Ok(Some(V::D(self.to_d(&vals[0], args[0].span)?)));
        }
        if let Some(f) = un {
            if vals.len() != 1 || !(any_d || portable_f32) {
                return Ok(None);
            }
            return Ok(Some(match &vals[0] {
                V::D(x) => V::D(f(&mut self.b, *x)),
                V::Vec(n, l) => {
                    let mut o = *l;
                    for k in 0..*n as usize {
                        let d = self.b.f2d(l[k]);
                        let r = f(&mut self.b, d);
                        o[k] = self.b.d2f(r);
                    }
                    V::Vec(*n, o)
                }
                v => {
                    let x = self.to_f(v, args[0].span)?;
                    let d = self.b.f2d(x);
                    let r = f(&mut self.b, d);
                    V::F(self.b.d2f(r))
                }
            }));
        }
        if let Some(f) = bin {
            if vals.len() != 2 || !(any_d || portable_f32) {
                return Ok(None);
            }
            if vals.iter().any(|v| matches!(v, V::Vec(..))) {
                return err(span, format!("`{}` on vectors: apply it per component", name));
            }
            if any_d {
                let x = self.to_d(&vals[0], args[0].span)?;
                let y = self.to_d(&vals[1], args[1].span)?;
                return Ok(Some(V::D(f(&mut self.b, x, y))));
            }
            let x = self.to_f(&vals[0], args[0].span)?;
            let y = self.to_f(&vals[1], args[1].span)?;
            let x = self.b.f2d(x);
            let y = self.b.f2d(y);
            let r = f(&mut self.b, x, y);
            return Ok(Some(V::F(self.b.d2f(r))));
        }
        if any_d && matches!(name, "clamp" | "mix" | "lerp") {
            if vals.len() != 3 {
                return err(span, format!("`{}` takes 3 arguments", name));
            }
            let x = self.to_d(&vals[0], args[0].span)?;
            let y = self.to_d(&vals[1], args[1].span)?;
            let z = self.to_d(&vals[2], args[2].span)?;
            let b = &mut self.b;
            return Ok(Some(V::D(if name == "clamp" {
                let m = b.def(Ty::F64, Op::Bin(Bin::MaxD, x, y));
                b.def(Ty::F64, Op::Bin(Bin::MinD, m, z))
            } else {
                let d = b.def(Ty::F64, Op::Bin(Bin::SubD, y, x));
                let d = b.def(Ty::F64, Op::Bin(Bin::MulD, d, z));
                b.def(Ty::F64, Op::Bin(Bin::AddD, x, d))
            })));
        }
        Ok(None)
    }

    fn call_expr(&mut self, name: &str, args: &[Expr], span: Span) -> LResult<V> {
        if name == "emit" && self.domain == Domain::Kernel && !self.fns.contains_key("emit") {
            return self.kernel_emit(args, span);
        }
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
        if let Some(v) = self.f64_call(name, &vals, args, span)? {
            return Ok(v);
        }
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
            "atan" => Some(|b, x| b.atan(x)),
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
                V::Vec(n, l) => {
                    let mut o = *l;
                    for k in 0..*n as usize {
                        o[k] = f(&mut self.b, l[k]);
                    }
                    V::Vec(*n, o)
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
                if let Some(n) = self.vec_width(&[&vals[0], &vals[1]], span)? {
                    let la = self.to_vec(&vals[0], n, span)?;
                    let lb = self.to_vec(&vals[1], n, span)?;
                    let mut o = la;
                    for k in 0..n as usize {
                        o[k] = self.b.fb(op, la[k], lb[k]);
                    }
                    return Ok(V::Vec(n, o));
                }
                let v = f_args(self, &vals)?;
                Ok(V::F(self.b.fb(op, v[0], v[1])))
            }
            "clamp" => {
                want(3)?;
                if let V::Vec(n, _) = vals[0] {
                    let l = self.to_vec(&vals[0], n, span)?;
                    let lo = self.to_vec(&vals[1], n, span)?;
                    let hi = self.to_vec(&vals[2], n, span)?;
                    let mut o = l;
                    for k in 0..n as usize {
                        let x = self.b.fb(Bin::MaxF, l[k], lo[k]);
                        o[k] = self.b.fb(Bin::MinF, x, hi[k]);
                    }
                    return Ok(V::Vec(n, o));
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
                if let Some(n) = self.vec_width(&[&vals[0], &vals[1]], span)? {
                    let la = self.to_vec(&vals[0], n, span)?;
                    let lb = self.to_vec(&vals[1], n, span)?;
                    let mut o = la;
                    for k in 0..n as usize {
                        let d = self.b.sub(lb[k], la[k]);
                        let d = self.b.mul(d, t);
                        o[k] = self.b.add(la[k], d);
                    }
                    return Ok(V::Vec(n, o));
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
            "atan2" => {
                want(2)?;
                let v = f_args(self, &vals)?;
                Ok(V::F(self.b.atan2(v[0], v[1])))
            }
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
            "vec2" | "vec3" | "vec4" | "mat4" => {
                let w: u8 = match name {
                    "vec2" => 2,
                    "vec3" => 3,
                    "vec4" => 4,
                    _ => 16,
                };
                // One scalar broadcasts (a mat4 of one scalar is s * identity
                // is NOT implied: mat4(s) fills every lane); otherwise the
                // components concatenate (vec4(v3, 1.0), mat4(c0, c1, c2, c3)).
                if n == 1 {
                    if let V::Vec(m, l) = vals[0] {
                        if m == w {
                            return Ok(V::Vec(m, l));
                        }
                    } else {
                        let x = self.to_f(&vals[0], span)?;
                        return Ok(V::Vec(w, [x; 16]));
                    }
                }
                let mut out = Vec::new();
                for (v, a) in vals.iter().zip(args) {
                    match v {
                        V::Vec(m, l) => out.extend_from_slice(&l[..*m as usize]),
                        v => out.push(self.to_f(v, a.span)?),
                    }
                }
                if out.len() != w as usize {
                    return err(span, format!("{} needs {} components, got {}", name, w, out.len()));
                }
                Ok(V::Vec(w, lanes_of(&out)))
            }
            "dot" => {
                want(2)?;
                let n = self.vec_width(&[&vals[0], &vals[1]], span)?.unwrap_or(1);
                if n == 1 {
                    let v = f_args(self, &vals)?;
                    return Ok(V::F(self.b.mul(v[0], v[1])));
                }
                let a = self.to_vec(&vals[0], n, span)?;
                let b = self.to_vec(&vals[1], n, span)?;
                Ok(V::F(self.dot(&a, &b, n)))
            }
            "length" => {
                want(1)?;
                match &vals[0] {
                    V::Vec(n, l) => {
                        let d = self.dot(l, l, *n);
                        Ok(V::F(self.b.un(Ty::F32, Un::SqrtF, d)))
                    }
                    v => {
                        let x = self.to_f(v, span)?;
                        Ok(V::F(self.b.fabs(x)))
                    }
                }
            }
            "distance" => {
                want(2)?;
                let d = self.binary(BinOp::Sub, vals[0].clone(), vals[1].clone(), span)?;
                match &d {
                    V::Vec(n, l) => {
                        let s = self.dot(l, l, *n);
                        Ok(V::F(self.b.un(Ty::F32, Un::SqrtF, s)))
                    }
                    v => {
                        let x = self.to_f(v, span)?;
                        Ok(V::F(self.b.fabs(x)))
                    }
                }
            }
            "normalize" => {
                want(1)?;
                let V::Vec(n, l) = vals[0] else {
                    return err(span, "normalize() takes a vector");
                };
                // A zero vector stays zero.
                let d = self.dot(&l, &l, n);
                let len = self.b.un(Ty::F32, Un::SqrtF, d);
                let zero = self.b.cf(0.0);
                let is_zero = self.b.cmpf(Cmp::Eq, len, zero);
                let one = self.b.cf(1.0);
                let safe = self.b.sel(is_zero, one, len);
                let inv = self.b.div(one, safe);
                let mut o = l;
                for k in 0..n as usize {
                    o[k] = self.b.mul(l[k], inv);
                }
                Ok(V::Vec(n, o))
            }
            "cross" => {
                want(2)?;
                let a = self.to_vec(&vals[0], 3, span)?;
                let b = self.to_vec(&vals[1], 3, span)?;
                let m = |l: &mut Self, i: usize, j: usize| {
                    let p = l.b.mul(a[i], b[j]);
                    let q = l.b.mul(a[j], b[i]);
                    l.b.sub(p, q)
                };
                let x = m(self, 1, 2);
                let y = m(self, 2, 0);
                let z = m(self, 0, 1);
                Ok(V::Vec(3, lanes_of(&[x, y, z])))
            }
            "transpose" => {
                want(1)?;
                let l = self.to_vec(&vals[0], 16, span)?;
                let mut o = l;
                for c in 0..4 {
                    for r in 0..4 {
                        o[c * 4 + r] = l[r * 4 + c];
                    }
                }
                Ok(V::Vec(16, o))
            }
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
                    V::Place(Place { ty: T::Buf(_, stride, offset, _), region: Region::Buf(k), .. }) => {
                        // Whole elements in the host buffer.
                        let (stride, offset, k) = (*stride, *offset, *k);
                        let words = self.b.raw(Ty::I32, Op::BufLen(k), None);
                        let o = self.b.ci(offset as i32);
                        let words = self.b.ib(Bin::SubI, words, o);
                        let st = self.b.ci(stride.max(1) as i32);
                        Ok(V::I(self.b.ib(Bin::DivI, words, st)))
                    }
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
            "rand" | "noise" if self.domain == Domain::Kernel => {
                err(span, "kernels have no running random state: use hash01(i, k) or rand01(seed, i, k) for per-element randomness")
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
            _ if self.domain == Domain::Kernel && crate::host::find(name).is_some() => self.kernel_host_call(name, &vals, args, span),
            _ => {
                let hint = self.suggest(name, true);
                err(span, format!("unknown function `{}`{}", name, hint))
            }
        }
    }
}

fn ann_width(a: &TypeAnn) -> u8 {
    match a {
        TypeAnn::Vec2 => 2,
        TypeAnn::Vec3 => 3,
        TypeAnn::Vec4 => 4,
        _ => 16,
    }
}

/// One component of a vector (`x`, `r`, `0`, …).
fn single_lane(n: u8, field: &str) -> Option<usize> {
    match field {
        "0" => Some(0),
        "1" => Some(1),
        "2" if n >= 3 && n <= 4 => Some(2),
        "3" if n == 4 => Some(3),
        f if f.len() == 1 => lane_of(n, f.chars().next().unwrap()),
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

/// Is `name` assigned (as a whole or by component) anywhere in `body`?
fn assigns(body: &[Stmt], name: &str) -> bool {
    fn root(e: &Expr) -> Option<&str> {
        match &e.kind {
            ExprKind::Ident(n) => Some(n),
            ExprKind::Field(b, _) | ExprKind::Index(b, _) => root(b),
            _ => None,
        }
    }
    fn expr(e: &Expr, name: &str) -> bool {
        match &e.kind {
            ExprKind::If(arms, else_) => {
                arms.iter().any(|(c, b)| expr(c, name) || stmts(b, name)) || else_.as_ref().is_some_and(|b| stmts(b, name))
            }
            ExprKind::Match(s, arms) => expr(s, name) || arms.iter().any(|(_, b)| stmts(b, name)),
            ExprKind::Block(b) => stmts(b, name),
            ExprKind::Bin(_, a, b) | ExprKind::Index(a, b) | ExprKind::ArrayRepeat(a, b) => expr(a, name) || expr(b, name),
            ExprKind::Neg(a) | ExprKind::Not(a) | ExprKind::Field(a, _) => expr(a, name),
            ExprKind::Call(_, args) | ExprKind::ArrayList(args) => args.iter().any(|a| expr(a, name)),
            ExprKind::StructLit(_, f) => f.iter().any(|(_, a)| expr(a, name)),
            _ => false,
        }
    }
    fn stmts(b: &[Stmt], name: &str) -> bool {
        b.iter().any(|s| match s {
            Stmt::Assign { target, value, .. } => root(target) == Some(name) || expr(value, name),
            Stmt::Let { value, .. } => expr(value, name),
            Stmt::Expr(e) => expr(e, name),
            Stmt::For { from, to, body, .. } => expr(from, name) || expr(to, name) || stmts(body, name),
            Stmt::While { cond, body, .. } => expr(cond, name) || stmts(body, name),
            Stmt::Loop { body, .. } => stmts(body, name),
            Stmt::Return(Some(e), _) => expr(e, name),
            _ => false,
        })
    }
    stmts(body, name)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let cur = row[j];
            row[j] = (row[j] + 1).min(row[j - 1] + 1).min(prev + (a[i - 1] != b[j - 1]) as usize);
            prev = cur;
        }
    }
    row[b.len()]
}

const BUILTIN_FNS: &[&str] = &[
    "sin", "cos", "tan", "tanh", "atan", "atan2", "asin", "acos", "hypot", "cbrt", "f64", "exp", "exp2", "log", "ln", "log2", "log10", "sqrt", "abs", "floor", "ceil", "round", "trunc", "fract",
    "sign", "midi_to_hz", "db_to_gain", "min", "max", "clamp", "mix", "pow", "step", "smoothstep", "vec2", "int", "float", "len",
    "read", "read_cubic", "rand", "stop",
];

const INPUT_NAMES: &[&str] = &["sample_rate", "SR", "note", "freq", "gate", "velocity", "trigger"];
const BUILTIN_NAMES: &[&str] = &[
    "sample_rate", "SR", "note", "freq", "gate", "velocity", "trigger", "PI", "TAU", "E", "voice", "effect", "block", "init",
];

/// Lowers a parsed shader into its render (and optional init) programs.
fn new_lowerer(prelude_base: usize, domain: Domain) -> Lowerer {
    Lowerer {
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
        prelude_base,
        domain,
        kernel: Default::default(),
        portable: false,
        funcs_out: Vec::new(),
        func_cache: HashMap::new(),
        call_sites: None,
    }
}

pub fn lower(items: &[Item], prelude_base: usize) -> Result<Lowered, ShaderError> {
    let mut l = new_lowerer(prelude_base, Domain::Audio);
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
