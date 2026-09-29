//! AIR, the audio IR: a small typed **structured** IR between the audio
//! shader front end and the backends (see
//! local/agent_state/edits/design/AUDIO-SHADERS.md).
//!
//! - Values: SSA temporaries ([`Val`], defined once by [`Stmt::Def`]) and
//!   typed mutable locals ([`Var`], like wasm locals) for everything that
//!   crosses control flow. Types are `F32`, `I32` and `Bool` (0/1).
//! - Control flow is structured: [`Stmt::If`] with two blocks, and
//!   [`Stmt::Loop`] with an iteration **cap** (the loop exits when the cap
//!   is reached), `Break(d)`/`Continue(d)` naming the d-th enclosing loop.
//!   Every program therefore terminates in bounded time, and every backend
//!   (the interpreter, native code, wasm) maps it without a CFG rebuild.
//! - Memory: four word regions (ctx, per-instance state, shared tables,
//!   the per-call frame). An access names its region, a static word base,
//!   the extent of the object it lies in and an optional dynamic word
//!   offset, which every backend **clamps** into the extent. Indexing
//!   wraps before that ([`Op::Wrap`]), so the clamp is a safety net: a
//!   program cannot express an out-of-range address.
//! - Every op is total and has one exact meaning (IEEE f32 without
//!   fusion, wrapping i32, division by zero gives 0, min/max are selects),
//!   so backends are held BIT-IDENTICAL to [`run`], the reference.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Ty {
    F32,
    I32,
    Bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Val(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Var(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Region {
    /// Per-instance host words: sample rate, frame, parameter targets.
    Ctx,
    /// Per-voice (instrument) or per-node (effect) state.
    State,
    /// Tables filled once by `init()`; read-only on the audio path.
    Shared,
    /// Per-call scratch (local arrays and structs).
    Frame,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Un {
    NegF,
    AbsF,
    SqrtF,
    FloorF,
    CeilF,
    TruncF,
    /// Round half away from zero (Rust `f32::round`).
    RoundF,
    /// f32 -> i32, truncating, saturating, NaN -> 0 (Rust `as`).
    F2I,
    /// i32 -> f32, round to nearest (Rust `as`).
    I2F,
    /// Reinterpret f32 bits as i32.
    BitsFI,
    /// Reinterpret i32 bits as f32.
    BitsIF,
    NegI,
    NotB,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bin {
    AddF,
    SubF,
    MulF,
    DivF,
    /// `a < b ? a : b`.
    MinF,
    /// `a > b ? a : b`.
    MaxF,
    AddI,
    SubI,
    MulI,
    /// Wrapping; division by zero gives 0.
    DivI,
    /// Wrapping; `a % 0` gives `a`.
    RemI,
    AndI,
    OrI,
    XorI,
    /// Shift amounts are taken mod 32.
    ShlI,
    ShrI,
    ShrUI,
    AndB,
    OrB,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Op {
    ConstF(f32),
    ConstI(i32),
    ConstB(bool),
    Get(Var),
    Un(Un, Val),
    Bin(Bin, Val, Val),
    CmpF(Cmp, Val, Val),
    /// Integer (or bool) compare.
    CmpI(Cmp, Val, Val),
    /// `c ? a : b` (any type; both sides already evaluated).
    Sel(Val, Val, Val),
    /// Euclidean `x mod len` (len > 0) -> i32 in `0..len`.
    Wrap(Val, u32),
    /// Reads word `base + clamp(off, 0, extent - 1)` of `region`.
    Load {
        region: Region,
        base: u32,
        extent: u32,
        off: Option<Val>,
    },
    /// Input channel `ch` at frame `clamp(idx, 0, n - 1)`.
    In { ch: u8, idx: Val },
    /// The frame count of this call (i32).
    FrameCount,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Def(Val, Op),
    Set(Var, Val),
    Store {
        region: Region,
        base: u32,
        extent: u32,
        off: Option<Val>,
        val: Val,
    },
    /// Output channel `ch` at frame `clamp(idx, 0, n - 1)` += val.
    Out { ch: u8, idx: Val, val: Val },
    If(Val, Block, Block),
    Loop { cap: u32, body: Block },
    Break(u32),
    Continue(u32),
}

pub type Block = Vec<Stmt>;

/// One compiled entry (the render program or the table `init`).
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub vals: Vec<Ty>,
    pub vars: Vec<Ty>,
    pub body: Block,
    pub frame_words: u32,
}

impl Program {
    /// Registers the interpreter needs (vals + vars + frame).
    pub fn scratch_words(&self) -> usize {
        self.vals.len() + self.vars.len() + self.frame_words as usize
    }

    /// Static instruction count weighted by loop caps: a bound on the work
    /// of one call per frame of the outer sample loop.
    pub fn cost(&self) -> u64 {
        fn walk(b: &Block, depth: u32) -> u64 {
            let mut sum = 0u64;
            for s in b {
                sum = sum.saturating_add(match s {
                    Stmt::If(_, t, e) => 1 + walk(t, depth).max(walk(e, depth)),
                    Stmt::Loop { cap, body } => {
                        // The outermost loop is the per-frame loop: count one frame.
                        let reps = if depth == 0 { 1 } else { *cap as u64 };
                        reps.saturating_mul(walk(body, depth + 1) + 2)
                    }
                    _ => 1,
                });
            }
            sum
        }
        walk(&self.body, 0)
    }
}

// =========================================================================
// Op semantics (shared by the interpreter and constant folding)
// =========================================================================

#[inline(always)]
fn f(x: u32) -> f32 {
    f32::from_bits(x)
}

#[inline(always)]
fn i(x: u32) -> i32 {
    x as i32
}

#[inline(always)]
pub fn eval_un(op: Un, a: u32) -> u32 {
    match op {
        Un::NegF => (-f(a)).to_bits(),
        Un::AbsF => f(a).abs().to_bits(),
        Un::SqrtF => f(a).sqrt().to_bits(),
        Un::FloorF => f(a).floor().to_bits(),
        Un::CeilF => f(a).ceil().to_bits(),
        Un::TruncF => f(a).trunc().to_bits(),
        Un::RoundF => f(a).round().to_bits(),
        Un::F2I => (f(a) as i32) as u32,
        Un::I2F => (i(a) as f32).to_bits(),
        Un::BitsFI | Un::BitsIF => a,
        Un::NegI => i(a).wrapping_neg() as u32,
        Un::NotB => (a == 0) as u32,
    }
}

#[inline(always)]
pub fn eval_bin(op: Bin, a: u32, b: u32) -> u32 {
    match op {
        Bin::AddF => (f(a) + f(b)).to_bits(),
        Bin::SubF => (f(a) - f(b)).to_bits(),
        Bin::MulF => (f(a) * f(b)).to_bits(),
        Bin::DivF => (f(a) / f(b)).to_bits(),
        Bin::MinF => {
            if f(a) < f(b) {
                a
            } else {
                b
            }
        }
        Bin::MaxF => {
            if f(a) > f(b) {
                a
            } else {
                b
            }
        }
        Bin::AddI => i(a).wrapping_add(i(b)) as u32,
        Bin::SubI => i(a).wrapping_sub(i(b)) as u32,
        Bin::MulI => i(a).wrapping_mul(i(b)) as u32,
        Bin::DivI => {
            if b == 0 {
                0
            } else {
                i(a).wrapping_div(i(b)) as u32
            }
        }
        Bin::RemI => {
            if b == 0 {
                a
            } else {
                i(a).wrapping_rem(i(b)) as u32
            }
        }
        Bin::AndI | Bin::AndB => a & b,
        Bin::OrI | Bin::OrB => a | b,
        Bin::XorI => a ^ b,
        Bin::ShlI => i(a).wrapping_shl(b) as u32,
        Bin::ShrI => i(a).wrapping_shr(b) as u32,
        Bin::ShrUI => a.wrapping_shr(b),
    }
}

#[inline(always)]
pub fn eval_cmp_f(cc: Cmp, a: u32, b: u32) -> u32 {
    let (a, b) = (f(a), f(b));
    (match cc {
        Cmp::Lt => a < b,
        Cmp::Le => a <= b,
        Cmp::Gt => a > b,
        Cmp::Ge => a >= b,
        Cmp::Eq => a == b,
        Cmp::Ne => a != b,
    }) as u32
}

#[inline(always)]
pub fn eval_cmp_i(cc: Cmp, a: u32, b: u32) -> u32 {
    let (a, b) = (i(a), i(b));
    (match cc {
        Cmp::Lt => a < b,
        Cmp::Le => a <= b,
        Cmp::Gt => a > b,
        Cmp::Ge => a >= b,
        Cmp::Eq => a == b,
        Cmp::Ne => a != b,
    }) as u32
}

#[inline(always)]
pub fn eval_wrap(x: u32, len: u32) -> u32 {
    if len.is_power_of_two() {
        x & (len - 1)
    } else {
        let r = i(x).wrapping_rem(len as i32);
        (if r < 0 { r + len as i32 } else { r }) as u32
    }
}

#[inline(always)]
pub fn clamp_off(off: u32, extent: u32) -> u32 {
    off.min(extent - 1)
}

// =========================================================================
// The reference interpreter
// =========================================================================

/// The memory and I/O a call runs against. Every I/O slice holds at least
/// `n` frames.
pub struct Io<'a> {
    pub ins: [&'a [f32]; 2],
    pub outs: [&'a mut [f32]; 2],
}

pub struct Mem<'a> {
    pub ctx: &'a mut [u32],
    pub state: &'a mut [u32],
    pub shared: &'a mut [u32],
}

enum Flow {
    Next,
    Break(u32),
    Continue(u32),
}

struct Interp<'a, 'm, 'i> {
    regs: &'a mut [u32],
    nvals: usize,
    frame: &'a mut [u32],
    mem: &'a mut Mem<'m>,
    io: &'a mut Io<'i>,
    n: u32,
}

impl<'a, 'm, 'i> Interp<'a, 'm, 'i> {
    #[inline(always)]
    fn r(&self, v: Val) -> u32 {
        self.regs[v.0 as usize]
    }

    fn region(&mut self, region: Region) -> &mut [u32] {
        match region {
            Region::Ctx => self.mem.ctx,
            Region::State => self.mem.state,
            Region::Shared => self.mem.shared,
            Region::Frame => self.frame,
        }
    }

    fn eval(&mut self, op: &Op) -> u32 {
        match *op {
            Op::ConstF(x) => x.to_bits(),
            Op::ConstI(x) => x as u32,
            Op::ConstB(x) => x as u32,
            Op::Get(v) => self.regs[self.nvals + v.0 as usize],
            Op::Un(u, a) => eval_un(u, self.r(a)),
            Op::Bin(b, x, y) => eval_bin(b, self.r(x), self.r(y)),
            Op::CmpF(cc, x, y) => eval_cmp_f(cc, self.r(x), self.r(y)),
            Op::CmpI(cc, x, y) => eval_cmp_i(cc, self.r(x), self.r(y)),
            Op::Sel(c, x, y) => {
                if self.r(c) != 0 {
                    self.r(x)
                } else {
                    self.r(y)
                }
            }
            Op::Wrap(x, len) => eval_wrap(self.r(x), len),
            Op::Load { region, base, extent, off } => {
                let off = match off {
                    Some(o) => clamp_off(self.r(o), extent),
                    None => 0,
                };
                self.region(region)[(base + off) as usize]
            }
            Op::In { ch, idx } => {
                let at = clamp_off(self.r(idx), self.n) as usize;
                self.io.ins[ch as usize][at].to_bits()
            }
            Op::FrameCount => self.n,
        }
    }

    fn block(&mut self, b: &Block) -> Flow {
        for s in b {
            match s {
                Stmt::Def(v, op) => {
                    let x = self.eval(op);
                    self.regs[v.0 as usize] = x;
                }
                Stmt::Set(var, v) => {
                    let x = self.r(*v);
                    self.regs[self.nvals + var.0 as usize] = x;
                }
                Stmt::Store { region, base, extent, off, val } => {
                    let off = match off {
                        Some(o) => clamp_off(self.r(*o), *extent),
                        None => 0,
                    };
                    let x = self.r(*val);
                    self.region(*region)[(base + off) as usize] = x;
                }
                Stmt::Out { ch, idx, val } => {
                    let at = clamp_off(self.r(*idx), self.n) as usize;
                    let x = f32::from_bits(self.r(*val));
                    let out = &mut self.io.outs[*ch as usize][at];
                    *out += x;
                }
                Stmt::If(c, t, e) => {
                    let flow = if self.r(*c) != 0 { self.block(t) } else { self.block(e) };
                    if !matches!(flow, Flow::Next) {
                        return flow;
                    }
                }
                Stmt::Loop { cap, body } => {
                    let mut k = 0;
                    while k < *cap {
                        k += 1;
                        match self.block(body) {
                            Flow::Next | Flow::Continue(0) => {}
                            Flow::Break(0) => break,
                            Flow::Break(d) => return Flow::Break(d - 1),
                            Flow::Continue(d) => return Flow::Continue(d - 1),
                        }
                    }
                }
                Stmt::Break(d) => return Flow::Break(*d),
                Stmt::Continue(d) => return Flow::Continue(*d),
            }
        }
        Flow::Next
    }
}

/// Runs `p` for `n` frames. `scratch` holds `p.scratch_words()` words (the
/// frame region is zeroed by the program's own initializers, never read
/// before written).
pub fn run(p: &Program, scratch: &mut [u32], mem: &mut Mem, io: &mut Io, n: u32) {
    if n == 0 {
        return;
    }
    let nregs = p.vals.len() + p.vars.len();
    let (regs, frame) = scratch.split_at_mut(nregs);
    let mut interp = Interp { regs, nvals: p.vals.len(), frame, mem, io, n };
    interp.block(&p.body);
}

// =========================================================================
// Validation (types, loop depths, memory extents)
// =========================================================================

/// Checks the structural invariants every backend relies on. The front end
/// always produces valid programs; this guards hand-built ones and
/// front-end bugs (tests call it on every compiled program).
pub fn validate(p: &Program, sizes: &dyn Fn(Region) -> u32) -> Result<(), String> {
    let mut defined = vec![false; p.vals.len()];
    fn walk(
        p: &Program,
        b: &Block,
        depth: u32,
        defined: &mut Vec<bool>,
        sizes: &dyn Fn(Region) -> u32,
    ) -> Result<(), String> {
        let ty = |v: Val| p.vals[v.0 as usize];
        for s in b {
            let used: Vec<Val> = match s {
                Stmt::Def(_, op) => op_uses(op),
                Stmt::Set(_, v) => vec![*v],
                Stmt::Store { off, val, .. } => off.iter().copied().chain([*val]).collect(),
                Stmt::Out { idx, val, .. } => vec![*idx, *val],
                Stmt::If(c, _, _) => vec![*c],
                _ => vec![],
            };
            for u in &used {
                if !defined.get(u.0 as usize).copied().unwrap_or(false) {
                    return Err(format!("{:?} used before definition", u));
                }
            }
            match s {
                Stmt::Def(v, op) => {
                    if defined[v.0 as usize] {
                        return Err(format!("{:?} defined twice", v));
                    }
                    let want = op_ty(p, op);
                    if let Some(want) = want {
                        if want != ty(*v) {
                            return Err(format!("{:?} typed {:?}, op gives {:?}", v, ty(*v), want));
                        }
                    }
                    if let Op::Load { region, base, extent, .. } = op {
                        if *extent == 0 || base + extent > sizes(*region) {
                            return Err(format!("load outside {:?}", region));
                        }
                    }
                    defined[v.0 as usize] = true;
                }
                Stmt::Set(var, v) => {
                    if p.vars[var.0 as usize] != ty(*v) {
                        return Err(format!("{:?} set with a {:?}", var, ty(*v)));
                    }
                }
                Stmt::Store { region, base, extent, .. } => {
                    if *extent == 0 || base + extent > sizes(*region) {
                        return Err(format!("store outside {:?}", region));
                    }
                }
                Stmt::If(c, t, e) => {
                    if ty(*c) != Ty::Bool {
                        return Err("if on a non-bool".into());
                    }
                    walk(p, t, depth, defined, sizes)?;
                    walk(p, e, depth, defined, sizes)?;
                }
                Stmt::Loop { body, .. } => walk(p, body, depth + 1, defined, sizes)?,
                Stmt::Break(d) | Stmt::Continue(d) => {
                    if *d >= depth {
                        return Err("break/continue outside its loop".into());
                    }
                }
                Stmt::Out { .. } => {}
            }
        }
        Ok(())
    }
    walk(p, &p.body, 0, &mut defined, sizes)
}

/// The operands an op reads.
pub fn op_uses(op: &Op) -> Vec<Val> {
    match *op {
        Op::ConstF(_) | Op::ConstI(_) | Op::ConstB(_) | Op::Get(_) | Op::FrameCount => vec![],
        Op::Un(_, a) | Op::Wrap(a, _) => vec![a],
        Op::Bin(_, a, b) | Op::CmpF(_, a, b) | Op::CmpI(_, a, b) => vec![a, b],
        Op::Sel(c, a, b) => vec![c, a, b],
        Op::Load { off, .. } => off.into_iter().collect(),
        Op::In { idx, .. } => vec![idx],
    }
}

/// The result type an op implies (None: taken from its declaration).
pub fn op_ty(p: &Program, op: &Op) -> Option<Ty> {
    Some(match *op {
        Op::ConstF(_) => Ty::F32,
        Op::ConstI(_) | Op::Wrap(..) | Op::FrameCount => Ty::I32,
        Op::ConstB(_) | Op::CmpF(..) | Op::CmpI(..) => Ty::Bool,
        Op::Get(v) => p.vars[v.0 as usize],
        Op::Un(u, _) => match u {
            Un::F2I | Un::BitsFI | Un::NegI => Ty::I32,
            Un::NotB => Ty::Bool,
            _ => Ty::F32,
        },
        Op::Bin(b, _, _) => match b {
            Bin::AddF | Bin::SubF | Bin::MulF | Bin::DivF | Bin::MinF | Bin::MaxF => Ty::F32,
            Bin::AndB | Bin::OrB => Ty::Bool,
            _ => Ty::I32,
        },
        Op::Sel(_, a, _) => p.vals[a.0 as usize],
        Op::In { .. } => Ty::F32,
        Op::Load { .. } => return None,
    })
}
