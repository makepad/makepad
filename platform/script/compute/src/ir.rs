//! AIR, the audio IR: a small typed **structured** IR between the audio
//! shader front end and the backends (see
//! local/agent_state/edits/design/AUDIO-SHADERS.md).
//!
//! - Values: SSA temporaries ([`Val`], defined once by [`Stmt::Def`]) and
//!   typed mutable locals ([`Var`], like wasm locals) for everything that
//!   crosses control flow. Types are `F32`, `I32`, `Bool` (0/1) and
//!   `F64` (registers only: memory is 32-bit words, so an f64 is stored as
//!   its two words through [`Un::HiD`]/[`Un::LoD`] and [`Bin::MakeD`]).
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
//!   fusion except the explicit [`Op::Fma`], wrapping i32, division by zero gives 0, min/max are selects),
//!   so backends are held BIT-IDENTICAL to [`run`], the reference.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Ty {
    F32,
    I32,
    Bool,
    F64,
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
    /// A host buffer (geometry kernels' inputs and outputs), sized at run
    /// time: an access clamps to the buffer's real length, not `extent`.
    Buf(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
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
    NegD,
    AbsD,
    SqrtD,
    FloorD,
    CeilD,
    TruncD,
    /// Round half away from zero (Rust `f64::round`).
    RoundD,
    /// f32 -> f64 (exact).
    F2D,
    /// f64 -> f32, round to nearest.
    D2F,
    /// f64 -> i32, truncating, saturating, NaN -> 0 (Rust `as`).
    D2I,
    /// i32 -> f64 (exact).
    I2D,
    /// The high word of an f64's bits (i32).
    HiD,
    /// The low word of an f64's bits (i32).
    LoD,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
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
    AddD,
    SubD,
    MulD,
    DivD,
    /// `a < b ? a : b`.
    MinD,
    /// `a > b ? a : b`.
    MaxD,
    /// The f64 whose bits are `(hi << 32) | lo` (two i32 words).
    MakeD,
}

/// A fused multiply-add's form: one rounding of the exact result.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Fma {
    /// `a * b + c`
    Add,
    /// `c - a * b`
    SubFrom,
    /// `a * b - c`
    Sub,
    /// i32 `a * b + c`, wrapping (exact: fused in every math mode).
    MulAddI,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
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
    ConstD(f64),
    ConstI(i32),
    ConstB(bool),
    Get(Var),
    Un(Un, Val),
    Bin(Bin, Val, Val),
    CmpF(Cmp, Val, Val),
    /// Integer (or bool) compare.
    CmpI(Cmp, Val, Val),
    CmpD(Cmp, Val, Val),
    /// `c ? a : b` (any type; both sides already evaluated).
    Sel(Val, Val, Val),
    /// f32 fused multiply-add of `(a, b, c)`, rounded once (only `math:
    /// fast` kernels contain it: the fusion pass makes it).
    Fma(Fma, Val, Val, Val),
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
    /// The frame count of this call (i32); for kernels, the element count.
    FrameCount,
    /// Length in words of host buffer k (i32).
    BufLen(u8),
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
    /// A call of registered host function `f` ([`crate::host`]): scalar
    /// arguments, buffer slices, and results defined here (like `Def`s).
    CallHost { f: u16, args: Vec<Val>, slices: Vec<SliceArg>, rets: Vec<Val> },
    /// A call of function `f` of the program ([`Program::funcs`]): its
    /// params take `args`, then its body runs (sharing the regions and the
    /// frame: functions are not recursive, so each has frame words of its
    /// own), then `rets` are defined as its results.
    Call { f: u16, args: Vec<Val>, rets: Vec<Val> },
}

/// A host call's buffer slice: host buffer `buf`, `len` words from word
/// `off` (both clamped to the buffer at run time).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliceArg {
    pub buf: u8,
    pub off: Val,
    pub len: Val,
}

/// A host call's static cost: at its slices' constant lengths where they
/// are constants; a length known only at run time counts as 0 here (the
/// call's run-time limit, `K_HOST_LIMIT`, and admission bound it). An
/// unknown function costs the most any may.
pub fn host_cost(f: u16, lens: &[Option<u32>]) -> u64 {
    crate::host::get(f).map_or(u64::MAX / 4, |h| {
        let mut l = [0u32; 8];
        for (k, x) in lens.iter().enumerate().take(8) {
            if let Some(c) = x {
                l[k] = *c;
            }
        }
        h.cost_of(&l[..lens.len().min(8)]).saturating_add(8)
    })
}

/// The block with each host call replaced by a unit statement.
fn strip_calls(b: &Block) -> Block {
    b.iter()
        .map(|s| match s {
            Stmt::CallHost { .. } => Stmt::Break(u32::MAX),
            Stmt::If(c, t, e) => Stmt::If(*c, strip_calls(t), strip_calls(e)),
            Stmt::Loop { cap, body } => Stmt::Loop { cap: *cap, body: strip_calls(body) },
            s => s.clone(),
        })
        .collect()
}

/// Every ConstI of a program (for costs at constant slice lengths).
fn const_ints(p: &Program) -> std::collections::HashMap<u32, u32> {
    fn walk(b: &Block, out: &mut std::collections::HashMap<u32, u32>) {
        for s in b {
            match s {
                Stmt::Def(v, Op::ConstI(c)) => {
                    out.insert(v.0, *c as u32);
                }
                Stmt::If(_, t, e) => {
                    walk(t, out);
                    walk(e, out);
                }
                Stmt::Loop { body, .. } => walk(body, out),
                _ => {}
            }
        }
    }
    let mut out = std::collections::HashMap::new();
    walk(&p.body, &mut out);
    out
}

fn call_cost(f: u16, slices: &[SliceArg], consts: &std::collections::HashMap<u32, u32>) -> u64 {
    let lens: Vec<Option<u32>> = slices.iter().map(|x| consts.get(&x.len.0).copied()).collect();
    host_cost(f, &lens)
}

pub type Block = Vec<Stmt>;

/// One compiled entry (the render program or the table `init`).
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub vals: Vec<Ty>,
    pub vars: Vec<Ty>,
    pub body: Block,
    pub frame_words: u32,
    /// A function's parameters: values defined on entry (empty for an
    /// entry program).
    pub params: Vec<Val>,
    /// A function's results: values of its top-level block read at its end.
    pub results: Vec<Val>,
    /// The functions [`Stmt::Call`] names (an entry program's; a function's
    /// own list is empty: calls in it name the entry program's functions).
    pub funcs: Vec<Program>,
}

impl Program {
    /// Registers the interpreter needs (vals + vars + frame, then the
    /// high words of every register when the program uses f64).
    pub fn scratch_words(&self) -> usize {
        let regs = self.vals.len() + self.vars.len();
        regs + self.frame_words as usize + if self.uses_f64() { regs } else { 0 }
    }

    /// Any f64 value or variable (their high words need registers too).
    pub fn uses_f64(&self) -> bool {
        self.vals.iter().chain(&self.vars).any(|t| *t == Ty::F64)
    }

    /// Worst-case ops of one whole run: every loop at its cap (`init()`).
    pub fn total_cost(&self) -> u64 {
        fn walk(b: &Block, c: &std::collections::HashMap<u32, u32>, funcs: &[Program]) -> u64 {
            let mut sum = 0u64;
            for s in b {
                sum = sum.saturating_add(match s {
                    Stmt::If(_, t, e) => 1 + walk(t, c, funcs).max(walk(e, c, funcs)),
                    Stmt::Loop { cap, body } => (*cap as u64).saturating_mul(walk(body, c, funcs) + 2),
                    Stmt::CallHost { f, slices, .. } => call_cost(*f, slices, c),
                    Stmt::Call { f, args, .. } => funcs.get(*f as usize).map_or(u64::MAX / 4, |g| walk(&g.body, &const_ints(g), funcs).saturating_add(4 + args.len() as u64)),
                    _ => 1,
                });
            }
            sum
        }
        walk(&self.body, &const_ints(self), &self.funcs)
    }

    /// [`Program::cost`] with every host call counted as its call overhead
    /// only: the generated code's own work (what the per-element cap
    /// bounds; host components are bounded per call at run time).
    pub fn air_cost(&self) -> u64 {
        let mut p = Program { funcs: self.funcs.clone(), ..Default::default() };
        std::mem::swap(&mut p.body, &mut strip_calls(&self.body));
        p.cost()
    }

    /// Static instruction count weighted by loop caps: a bound on the work
    /// of one call per frame of the outer sample loop.
    pub fn cost(&self) -> u64 {
        fn walk(b: &Block, depth: u32, c: &std::collections::HashMap<u32, u32>, funcs: &[Program]) -> u64 {
            let mut sum = 0u64;
            for s in b {
                sum = sum.saturating_add(match s {
                    Stmt::If(_, t, e) => 1 + walk(t, depth, c, funcs).max(walk(e, depth, c, funcs)),
                    Stmt::Loop { cap, body } => {
                        // The outermost loop is the per-frame loop: count one frame.
                        let reps = if depth == 0 { 1 } else { *cap as u64 };
                        reps.saturating_mul(walk(body, depth + 1, c, funcs) + 2)
                    }
                    Stmt::CallHost { f, slices, .. } => call_cost(*f, slices, c),
                    // A call costs its function's body (its loops at their
                    // caps), plus passing its arguments.
                    Stmt::Call { f, args, .. } => funcs.get(*f as usize).map_or(u64::MAX / 4, |g| walk(&g.body, depth.max(1), &const_ints(g), funcs).saturating_add(4 + args.len() as u64)),
                    _ => 1,
                });
            }
            sum
        }
        walk(&self.body, 0, &const_ints(self), &self.funcs)
    }
}

// =========================================================================
// Op semantics (shared by the interpreter and constant folding)
// =========================================================================

#[inline(always)]
fn f(x: u64) -> f32 {
    f32::from_bits(x as u32)
}

#[inline(always)]
fn i(x: u64) -> i32 {
    x as u32 as i32
}

#[inline(always)]
fn d(x: u64) -> f64 {
    f64::from_bits(x)
}

#[inline(always)]
fn fw(x: f32) -> u64 {
    x.to_bits() as u64
}

#[inline(always)]
fn iw(x: i32) -> u64 {
    x as u32 as u64
}

/// An op on register bits: 32-bit types in the low word (the high word of
/// the result is 0), f64 in all 64 bits.
#[inline(always)]
pub fn eval_un(op: Un, a: u64) -> u64 {
    match op {
        Un::NegF => fw(-f(a)),
        Un::AbsF => fw(f(a).abs()),
        Un::SqrtF => fw(f(a).sqrt()),
        Un::FloorF => fw(f(a).floor()),
        Un::CeilF => fw(f(a).ceil()),
        Un::TruncF => fw(f(a).trunc()),
        Un::RoundF => fw(f(a).round()),
        Un::F2I => iw(f(a) as i32),
        Un::I2F => fw(i(a) as f32),
        Un::BitsFI | Un::BitsIF => a & 0xFFFF_FFFF,
        Un::NegI => iw(i(a).wrapping_neg()),
        Un::NotB => (a as u32 == 0) as u64,
        Un::NegD => (-d(a)).to_bits(),
        Un::AbsD => d(a).abs().to_bits(),
        Un::SqrtD => d(a).sqrt().to_bits(),
        Un::FloorD => d(a).floor().to_bits(),
        Un::CeilD => d(a).ceil().to_bits(),
        Un::TruncD => d(a).trunc().to_bits(),
        Un::RoundD => d(a).round().to_bits(),
        Un::F2D => (f(a) as f64).to_bits(),
        Un::D2F => fw(d(a) as f32),
        Un::D2I => iw(d(a) as i32),
        Un::I2D => (i(a) as f64).to_bits(),
        Un::HiD => a >> 32,
        Un::LoD => a & 0xFFFF_FFFF,
    }
}

#[inline(always)]
pub fn eval_bin(op: Bin, a: u64, b: u64) -> u64 {
    match op {
        Bin::AddF => fw(f(a) + f(b)),
        Bin::SubF => fw(f(a) - f(b)),
        Bin::MulF => fw(f(a) * f(b)),
        Bin::DivF => fw(f(a) / f(b)),
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
        Bin::AddI => iw(i(a).wrapping_add(i(b))),
        Bin::SubI => iw(i(a).wrapping_sub(i(b))),
        Bin::MulI => iw(i(a).wrapping_mul(i(b))),
        Bin::DivI => {
            if b as u32 == 0 {
                0
            } else {
                iw(i(a).wrapping_div(i(b)))
            }
        }
        Bin::RemI => {
            if b as u32 == 0 {
                a
            } else {
                iw(i(a).wrapping_rem(i(b)))
            }
        }
        Bin::AndI | Bin::AndB => a & b,
        Bin::OrI | Bin::OrB => a | b,
        Bin::XorI => a ^ b,
        Bin::ShlI => iw(i(a).wrapping_shl(b as u32)),
        Bin::ShrI => iw(i(a).wrapping_shr(b as u32)),
        Bin::ShrUI => (a as u32).wrapping_shr(b as u32) as u64,
        Bin::AddD => (d(a) + d(b)).to_bits(),
        Bin::SubD => (d(a) - d(b)).to_bits(),
        Bin::MulD => (d(a) * d(b)).to_bits(),
        Bin::DivD => (d(a) / d(b)).to_bits(),
        Bin::MinD => {
            if d(a) < d(b) {
                a
            } else {
                b
            }
        }
        Bin::MaxD => {
            if d(a) > d(b) {
                a
            } else {
                b
            }
        }
        Bin::MakeD => (a << 32) | (b & 0xFFFF_FFFF),
    }
}

#[inline(always)]
pub fn eval_fma(k: Fma, a: u64, b: u64, c: u64) -> u64 {
    if k == Fma::MulAddI {
        return iw(i(a).wrapping_mul(i(b)).wrapping_add(i(c)));
    }
    let (a, b, c) = (f(a), f(b), f(c));
    fw(match k {
        Fma::Add => a.mul_add(b, c),
        Fma::SubFrom => (-a).mul_add(b, c),
        Fma::Sub => a.mul_add(b, -c),
        Fma::MulAddI => unreachable!(),
    })
}

#[inline(always)]
fn cmp<T: PartialOrd>(cc: Cmp, a: T, b: T) -> u64 {
    (match cc {
        Cmp::Lt => a < b,
        Cmp::Le => a <= b,
        Cmp::Gt => a > b,
        Cmp::Ge => a >= b,
        Cmp::Eq => a == b,
        Cmp::Ne => a != b,
    }) as u64
}

#[inline(always)]
pub fn eval_cmp_f(cc: Cmp, a: u64, b: u64) -> u64 {
    cmp(cc, f(a), f(b))
}

#[inline(always)]
pub fn eval_cmp_d(cc: Cmp, a: u64, b: u64) -> u64 {
    cmp(cc, d(a), d(b))
}

#[inline(always)]
pub fn eval_cmp_i(cc: Cmp, a: u64, b: u64) -> u64 {
    cmp(cc, i(a), i(b))
}

#[inline(always)]
pub fn eval_wrap(x: u32, len: u32) -> u32 {
    if len.is_power_of_two() {
        x & (len - 1)
    } else {
        let r = (x as i32).wrapping_rem(len as i32);
        (if r < 0 { r + len as i32 } else { r }) as u32
    }
}

/// The signed magic multiplier and shift for division by a constant
/// `d` in 2..2^31 (Hacker's Delight 10-1): `x / d` (truncating) is
/// `q = mulhi(m, x) (+ x when m < 0); q >>= s (arithmetic); q + (x >>> 31)`.
/// The backends use it for `Wrap` and integer division by constants.
pub fn magic_s32(d: u32) -> (i32, u32) {
    debug_assert!((2..1 << 31).contains(&d));
    let two31: u32 = 1 << 31;
    let anc = two31 - 1 - two31 % d;
    let mut p = 31u32;
    let (mut q1, mut r1) = (two31 / anc, two31 - (two31 / anc) * anc);
    let (mut q2, mut r2) = (two31 / d, two31 - (two31 / d) * d);
    loop {
        p += 1;
        q1 = q1.wrapping_mul(2);
        r1 = r1.wrapping_mul(2);
        if r1 >= anc {
            q1 = q1.wrapping_add(1);
            r1 = r1.wrapping_sub(anc);
        }
        q2 = q2.wrapping_mul(2);
        r2 = r2.wrapping_mul(2);
        if r2 >= d {
            q2 = q2.wrapping_add(1);
            r2 = r2.wrapping_sub(d);
        }
        let delta = d - r2;
        if !(q1 < delta || (q1 == delta && r1 == 0)) {
            break;
        }
    }
    (q2.wrapping_add(1) as i32, p - 32)
}

/// `x / d` by [`magic_s32`] (the backends' sequence, for tests).
pub fn div_by_magic(x: i32, d: u32) -> i32 {
    let (m, s) = magic_s32(d);
    let mut q = ((x as i64 * m as i64) >> 32) as i32;
    if m < 0 {
        q = q.wrapping_add(x);
    }
    q >>= s;
    q + ((x as u32) >> 31) as i32
}

#[inline(always)]
pub fn clamp_off(off: u32, extent: u32) -> u32 {
    off.min(extent.saturating_sub(1))
}

/// Upper bounds (as u32) of i32 values built from `Wrap`, masks, unsigned
/// shifts and constant arithmetic: `Some(b)` means the value, read as
/// u32, is at most b (so it is also a non-negative i32 when b < 2^31).
pub fn bounds(p: &Program) -> Vec<Option<u32>> {
    let mut out = vec![None; p.vals.len()];
    let mut consts: Vec<Option<i32>> = vec![None; p.vals.len()];
    fn walk(b: &Block, out: &mut Vec<Option<u32>>, consts: &mut Vec<Option<i32>>) {
        for s in b {
            match s {
                Stmt::Def(v, op) => {
                    let pos = |c: Option<i32>| c.filter(|c| *c >= 0).map(|c| c as u32);
                    let r = match *op {
                        Op::ConstI(c) => {
                            consts[v.0 as usize] = Some(c);
                            pos(Some(c))
                        }
                        Op::Wrap(_, len) => Some(len - 1),
                        Op::Bin(Bin::AddI, a, b) => match (out[a.0 as usize], pos(consts[b.0 as usize]), pos(consts[a.0 as usize]), out[b.0 as usize]) {
                            (Some(x), Some(c), _, _) | (_, _, Some(c), Some(x)) => x.checked_add(c),
                            _ => None,
                        },
                        Op::Bin(Bin::MulI, a, b) => match (out[a.0 as usize], pos(consts[b.0 as usize]), pos(consts[a.0 as usize]), out[b.0 as usize]) {
                            (Some(x), Some(c), _, _) | (_, _, Some(c), Some(x)) => x.checked_mul(c),
                            _ => None,
                        },
                        Op::Bin(Bin::AndI, a, b) => pos(consts[b.0 as usize]).or(pos(consts[a.0 as usize])).or(out[a.0 as usize]).or(out[b.0 as usize]),
                        Op::Bin(Bin::ShrUI, a, b) => consts[b.0 as usize].map(|k| out[a.0 as usize].unwrap_or(u32::MAX) >> (k as u32 & 31)),
                        Op::Bin(Bin::ShlI, a, b) => match (out[a.0 as usize], consts[b.0 as usize]) {
                            (Some(x), Some(k)) if (0..31).contains(&k) => x.checked_shl(k as u32).filter(|y| y >> k == x && *y <= i32::MAX as u32),
                            _ => None,
                        },
                        Op::Fma(Fma::MulAddI, a, b, c) => {
                            let m = match (out[a.0 as usize], pos(consts[b.0 as usize]), pos(consts[a.0 as usize]), out[b.0 as usize]) {
                                (Some(x), Some(k), _, _) | (_, _, Some(k), Some(x)) => x.checked_mul(k),
                                _ => None,
                            };
                            m.zip(out[c.0 as usize].or(pos(consts[c.0 as usize]))).and_then(|(m, c)| m.checked_add(c))
                        }
                        Op::Sel(_, a, b) => match (out[a.0 as usize], out[b.0 as usize]) {
                            (Some(x), Some(y)) => Some(x.max(y)),
                            _ => None,
                        },
                        _ => None,
                    };
                    out[v.0 as usize] = r;
                }
                Stmt::If(_, t, e) => {
                    walk(t, out, consts);
                    walk(e, out, consts);
                }
                Stmt::Loop { body, .. } => walk(body, out, consts),
                _ => {}
            }
        }
    }
    walk(&p.body, &mut out, &mut consts);
    out
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

/// A host buffer as a run sees it. Several workers may use one buffer at
/// once (each writing only its own elements), so the interpreter never makes
/// a Rust reference over this memory: it reads and writes single words
/// through relaxed atomics.
#[derive(Clone, Copy, Debug)]
pub struct RawBuf {
    pub ptr: *mut u32,
    /// Length in words.
    pub len: usize,
    pub writable: bool,
}

// SAFETY: a RawBuf is only dereferenced word by word through atomics, at
// clamped indices below `len`, by the runs its owner hands it to.
unsafe impl Send for RawBuf {}
unsafe impl Sync for RawBuf {}

impl RawBuf {
    /// Word `at` (0 past the end).
    pub fn load_word(&self, at: usize) -> u32 {
        self.load(at)
    }

    /// Writes word `at` (ignored past the end or when read-only).
    pub fn store_word(&self, at: usize, x: u32) {
        self.store(at, x)
    }

    #[inline(always)]
    fn load(&self, at: usize) -> u32 {
        if at >= self.len {
            return 0;
        }
        // SAFETY: at < len, and the owner keeps ptr..ptr+len alive for the run.
        unsafe { (*(self.ptr.add(at) as *const std::sync::atomic::AtomicU32)).load(std::sync::atomic::Ordering::Relaxed) }
    }

    #[inline(always)]
    fn store(&self, at: usize, x: u32) {
        if at >= self.len || !self.writable {
            return;
        }
        // SAFETY: as in `load`; the buffer was bound writable.
        unsafe { (*(self.ptr.add(at) as *const std::sync::atomic::AtomicU32)).store(x, std::sync::atomic::Ordering::Relaxed) }
    }
}

/// Shared tables: written only by `init()`; every other program reads them.
pub enum Shared<'a> {
    Read(&'a [u32]),
    Write(&'a mut [u32]),
}

impl Shared<'_> {
    fn get(&self, at: usize) -> u32 {
        match self {
            Shared::Read(s) => s.get(at).copied().unwrap_or(0),
            Shared::Write(s) => s.get(at).copied().unwrap_or(0),
        }
    }
}

pub struct Mem<'a> {
    /// The run's own ctx (params in, reduce/overflow words out).
    pub ctx: &'a mut [u32],
    pub state: &'a mut [u32],
    pub shared: Shared<'a>,
    /// Host buffers (each at least one word long).
    pub bufs: &'a [RawBuf],
}

/// A buffer access: `base + off` computed without wrapping (64-bit), then
/// clamped to the last word. The same in every backend.
#[inline(always)]
pub fn buf_index(base: u32, off: Option<u32>, len: usize) -> usize {
    ((base as u64 + off.unwrap_or(0) as u64) as usize).min(len.saturating_sub(1))
}

enum Flow {
    Next,
    Break(u32),
    Continue(u32),
}

struct Interp<'a, 'm, 'i> {
    regs: &'a mut [u32],
    /// High words of the registers (empty unless the program uses f64).
    hi: &'a mut [u32],
    nvals: usize,
    frame: &'a mut [u32],
    mem: &'a mut Mem<'m>,
    io: &'a mut Io<'i>,
    n: u32,
    /// The entry program's functions, and a register file for each (calls
    /// do not recurse: one at a time per function), taken while it runs.
    funcs: &'a [Program],
    func_regs: &'a mut Vec<(Vec<u32>, Vec<u32>)>,
}

impl<'a, 'm, 'i> Interp<'a, 'm, 'i> {
    #[inline(always)]
    fn r(&self, v: Val) -> u64 {
        self.reg(v.0 as usize)
    }

    #[inline(always)]
    fn reg(&self, at: usize) -> u64 {
        let lo = self.regs[at] as u64;
        match self.hi.get(at) {
            Some(h) => lo | (*h as u64) << 32,
            None => lo,
        }
    }

    #[inline(always)]
    fn set_reg(&mut self, at: usize, x: u64) {
        self.regs[at] = x as u32;
        if let Some(h) = self.hi.get_mut(at) {
            *h = (x >> 32) as u32;
        }
    }

    /// Reads word `at` of an owned region (clamped callers; out of range
    /// reads 0 so a malformed program cannot panic here).
    fn read_word(&self, region: Region, at: usize) -> u32 {
        match region {
            Region::Ctx => self.mem.ctx.get(at).copied().unwrap_or(0),
            Region::State => self.mem.state.get(at).copied().unwrap_or(0),
            Region::Shared => self.mem.shared.get(at),
            Region::Frame => self.frame.get(at).copied().unwrap_or(0),
            Region::Buf(k) => self.mem.bufs.get(k as usize).map_or(0, |b| b.load(at)),
        }
    }

    fn write_word(&mut self, region: Region, at: usize, x: u32) {
        let slot = match region {
            Region::Ctx => self.mem.ctx.get_mut(at),
            Region::State => self.mem.state.get_mut(at),
            Region::Shared => match &mut self.mem.shared {
                Shared::Write(s) => s.get_mut(at),
                Shared::Read(_) => None,
            },
            Region::Frame => self.frame.get_mut(at),
            Region::Buf(k) => {
                if let Some(b) = self.mem.bufs.get(k as usize) {
                    b.store(at, x);
                }
                None
            }
        };
        if let Some(w) = slot {
            *w = x;
        }
    }

    fn eval(&mut self, op: &Op) -> u64 {
        match *op {
            Op::ConstF(x) => x.to_bits() as u64,
            Op::ConstD(x) => x.to_bits(),
            Op::ConstI(x) => x as u32 as u64,
            Op::ConstB(x) => x as u64,
            Op::Get(v) => self.reg(self.nvals + v.0 as usize),
            Op::Un(u, a) => eval_un(u, self.r(a)),
            Op::Bin(b, x, y) => eval_bin(b, self.r(x), self.r(y)),
            Op::CmpF(cc, x, y) => eval_cmp_f(cc, self.r(x), self.r(y)),
            Op::CmpD(cc, x, y) => eval_cmp_d(cc, self.r(x), self.r(y)),
            Op::CmpI(cc, x, y) => eval_cmp_i(cc, self.r(x), self.r(y)),
            Op::Sel(c, x, y) => {
                if self.r(c) != 0 {
                    self.r(x)
                } else {
                    self.r(y)
                }
            }
            Op::Wrap(x, len) => eval_wrap(self.r(x) as u32, len) as u64,
            Op::Fma(k, a, b, c) => eval_fma(k, self.r(a), self.r(b), self.r(c)),
            Op::Load { region: Region::Buf(k), base, off, .. } => {
                let Some(b) = self.mem.bufs.get(k as usize) else { return 0 };
                if b.len == 0 {
                    return 0;
                }
                b.load(buf_index(base, off.map(|o| self.r(o) as u32), b.len)) as u64
            }
            Op::BufLen(k) => self.mem.bufs.get(k as usize).map_or(0, |b| b.len as u32) as u64,
            Op::Load { region, base, extent, off } => {
                let off = match off {
                    Some(o) => clamp_off(self.r(o) as u32, extent),
                    None => 0,
                };
                self.read_word(region, base as usize + off as usize) as u64
            }
            Op::In { ch, idx } => {
                let at = clamp_off(self.r(idx) as u32, self.n) as usize;
                self.io.ins[ch as usize][at].to_bits() as u64
            }
            Op::FrameCount => self.n as u64,
        }
    }

    fn block(&mut self, b: &Block) -> Flow {
        for s in b {
            match s {
                Stmt::Def(v, op) => {
                    let x = self.eval(op);
                    self.set_reg(v.0 as usize, x);
                }
                Stmt::Set(var, v) => {
                    let x = self.r(*v);
                    self.set_reg(self.nvals + var.0 as usize, x);
                }
                Stmt::Store { region: Region::Buf(k), base, off, val, .. } => {
                    let x = self.r(*val) as u32;
                    let o = off.map(|o| self.r(o) as u32);
                    if let Some(b) = self.mem.bufs.get(*k as usize) {
                        if b.len > 0 {
                            b.store(buf_index(*base, o, b.len), x);
                        }
                    }
                }
                Stmt::Store { region, base, extent, off, val } => {
                    let off = match off {
                        Some(o) => clamp_off(self.r(*o) as u32, *extent),
                        None => 0,
                    };
                    let x = self.r(*val) as u32;
                    self.write_word(*region, *base as usize + off as usize, x);
                }
                Stmt::Out { ch, idx, val } => {
                    let at = clamp_off(self.r(*idx) as u32, self.n) as usize;
                    let x = f32::from_bits(self.r(*val) as u32);
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
                Stmt::Call { f, args, rets } => {
                    let Some(g) = self.funcs.get(*f as usize) else { continue };
                    let nregs = g.vals.len() + g.vars.len();
                    let wide = g.uses_f64();
                    let (mut regs, mut hi) = std::mem::take(&mut self.func_regs[*f as usize]);
                    // Fresh (zero) variables and values per call.
                    regs.clear();
                    regs.resize(nregs, 0);
                    hi.clear();
                    hi.resize(if wide { nregs } else { 0 }, 0);
                    for (pv, a) in g.params.iter().zip(args) {
                        let x = self.r(*a);
                        regs[pv.0 as usize] = x as u32;
                        if wide {
                            hi[pv.0 as usize] = (x >> 32) as u32;
                        }
                    }
                    let out: Vec<u64> = {
                        let mut callee = Interp { regs: &mut regs, hi: &mut hi, nvals: g.vals.len(), frame: &mut *self.frame, mem: &mut *self.mem, io: &mut *self.io, n: self.n, funcs: self.funcs, func_regs: &mut *self.func_regs };
                        callee.block(&g.body);
                        g.results.iter().map(|r| callee.r(*r)).collect()
                    };
                    self.func_regs[*f as usize] = (regs, hi);
                    for (v, x) in rets.iter().zip(out) {
                        self.set_reg(v.0 as usize, x);
                    }
                }
                Stmt::CallHost { f, args, slices, rets } => {
                    let mut a = [0u32; 16];
                    for (k, v) in args.iter().enumerate().take(16) {
                        a[k] = self.r(*v) as u32;
                    }
                    let mut sl = [crate::host::SliceRaw { buf: 0, off: 0, len: 0 }; 8];
                    for (k, x) in slices.iter().enumerate().take(8) {
                        sl[k] = crate::host::SliceRaw { buf: x.buf as u32, off: self.r(x.off) as u32, len: self.r(x.len) as u32 };
                    }
                    let mut out = [0u32; 1];
                    let nr = rets.len().min(1);
                    let limit = self.mem.ctx.get(crate::lower::kernel::K_HOST_LIMIT as usize).copied().unwrap_or(0) as u64;
                    let ok = crate::host::invoke_limited(*f, &a[..args.len().min(16)], &sl[..slices.len().min(8)], &mut out[..nr], self.mem.bufs, limit);
                    if !ok {
                        if let Some(w) = self.mem.ctx.get_mut(crate::lower::kernel::K_HOST_ERR as usize) {
                            *w = 1;
                        }
                    }
                    for (k, v) in rets.iter().enumerate().take(1) {
                        self.set_reg(v.0 as usize, out[k] as u64);
                    }
                }
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
    let (regs, rest) = scratch.split_at_mut(nregs);
    let (frame, hi) = rest.split_at_mut(p.frame_words as usize);
    let hi = if p.uses_f64() { hi } else { &mut [] };
    let mut func_regs = vec![(Vec::new(), Vec::new()); p.funcs.len()];
    let mut interp = Interp { regs, hi, nvals: p.vals.len(), frame, mem, io, n, funcs: &p.funcs, func_regs: &mut func_regs };
    interp.block(&p.body);
}

// =========================================================================
// Validation (types, loop depths, memory extents)
// =========================================================================

/// What a program may touch: region sizes in words, which host buffers
/// exist and which are writable, whether shared tables are writable (only
/// `init()`), whether audio I/O ops are allowed.
#[derive(Clone, Debug, Default)]
pub struct Regions {
    pub ctx: u32,
    pub state: u32,
    pub shared: u32,
    pub frame: u32,
    pub shared_writable: bool,
    /// Writable flag per host buffer index.
    pub bufs: Vec<bool>,
    /// Audio I/O ops are allowed (audio programs; they never call host
    /// functions, which only kernels may).
    pub io: bool,
}

impl Regions {
    fn size(&self, r: Region) -> Option<u32> {
        match r {
            Region::Ctx => Some(self.ctx),
            Region::State => Some(self.state),
            Region::Shared => Some(self.shared),
            Region::Frame => Some(self.frame),
            Region::Buf(_) => None,
        }
    }
}

/// Checks every invariant the backends rely on, without ever panicking on
/// a malformed program: identifiers in range, types of every operand,
/// values used only where defined (per branch and loop scope), accesses
/// inside real region sizes (checked arithmetic), existing host buffers,
/// writes only to writable memory, break/continue depths. The front end
/// always produces valid programs; this is the independent barrier in
/// front of native code generation.
pub fn validate(p: &Program, regions: &Regions) -> Result<(), String> {
    struct V<'a> {
        p: &'a Program,
        r: &'a Regions,
        visible: Vec<bool>,
        ever: Vec<bool>,
        scope: Vec<u32>,
        funcs: &'a [Program],
    }
    impl<'a> V<'a> {
        fn ty(&self, v: Val) -> Result<Ty, String> {
            self.p.vals.get(v.0 as usize).copied().ok_or_else(|| format!("{:?} out of range", v))
        }
        fn var_ty(&self, v: Var) -> Result<Ty, String> {
            self.p.vars.get(v.0 as usize).copied().ok_or_else(|| format!("{:?} out of range", v))
        }
        fn used(&self, v: Val, want: Option<Ty>) -> Result<Ty, String> {
            let t = self.ty(v)?;
            if !self.visible[v.0 as usize] {
                return Err(format!("{:?} used where it is not defined", v));
            }
            if let Some(w) = want {
                if t != w {
                    return Err(format!("{:?} is {:?}, expected {:?}", v, t, w));
                }
            }
            Ok(t)
        }
        fn access(&self, region: Region, base: u32, extent: u32, off: Option<Val>, write: bool) -> Result<(), String> {
            if let Some(o) = off {
                self.used(o, Some(Ty::I32))?;
            }
            match region {
                Region::Buf(k) => {
                    let Some(w) = self.r.bufs.get(k as usize) else {
                        return Err(format!("no host buffer {}", k));
                    };
                    if write && !*w {
                        return Err(format!("write to read-only buffer {}", k));
                    }
                }
                r => {
                    let size = self.r.size(r).unwrap_or(0);
                    let end = base.checked_add(extent).ok_or("access extent overflows")?;
                    if extent == 0 || end > size {
                        return Err(format!("access outside {:?} ({}..{} of {})", r, base, end, size));
                    }
                    if write && r == Region::Shared && !self.r.shared_writable {
                        return Err("write to read-only shared tables".into());
                    }
                }
            }
            Ok(())
        }
        fn op(&self, v: Val, op: &Op) -> Result<(), String> {
            use Ty::*;
            let t = self.ty(v)?;
            let want = match *op {
                Op::ConstF(_) => F32,
                Op::ConstD(_) => F64,
                Op::ConstI(_) => I32,
                Op::ConstB(_) => Bool,
                Op::Get(var) => self.var_ty(var)?,
                Op::Un(u, a) => {
                    let (inp, out) = un_types(u);
                    self.used(a, Some(inp))?;
                    out
                }
                Op::Bin(b, x, y) => {
                    let (inp, out) = bin_types(b);
                    self.used(x, Some(inp))?;
                    self.used(y, Some(inp))?;
                    out
                }
                Op::CmpF(_, x, y) => {
                    self.used(x, Some(F32))?;
                    self.used(y, Some(F32))?;
                    Bool
                }
                Op::CmpD(_, x, y) => {
                    self.used(x, Some(F64))?;
                    self.used(y, Some(F64))?;
                    Bool
                }
                Op::CmpI(_, x, y) => {
                    let tx = self.used(x, None)?;
                    self.used(y, Some(tx))?;
                    if tx == F32 || tx == F64 {
                        return Err("integer compare of floats".into());
                    }
                    Bool
                }
                Op::Sel(c, x, y) => {
                    self.used(c, Some(Bool))?;
                    let tx = self.used(x, None)?;
                    self.used(y, Some(tx))?;
                    tx
                }
                Op::Fma(k, a, b, c) => {
                    let t = if k == Fma::MulAddI { I32 } else { F32 };
                    self.used(a, Some(t))?;
                    self.used(b, Some(t))?;
                    self.used(c, Some(t))?;
                    t
                }
                Op::Wrap(x, len) => {
                    self.used(x, Some(I32))?;
                    if len == 0 {
                        return Err("wrap by 0".into());
                    }
                    I32
                }
                Op::Load { region, base, extent, off } => {
                    self.access(region, base, extent, off, false)?;
                    if t == F64 {
                        return Err("memory words are 32-bit: an f64 is loaded as two words".into());
                    }
                    t
                }
                Op::In { idx, .. } => {
                    if !self.r.io {
                        return Err("audio input in a program without audio I/O".into());
                    }
                    self.used(idx, Some(I32))?;
                    F32
                }
                Op::FrameCount => I32,
                Op::BufLen(k) => {
                    if k as usize >= self.r.bufs.len() {
                        return Err(format!("no host buffer {}", k));
                    }
                    I32
                }
            };
            if want != t {
                return Err(format!("{:?} typed {:?}, its op gives {:?}", v, t, want));
            }
            Ok(())
        }
        fn block(&mut self, b: &Block, depth: u32) -> Result<(), String> {
            let mark = self.scope.len();
            for s in b {
                match s {
                    Stmt::Def(v, op) => {
                        self.ty(*v)?;
                        if self.ever[v.0 as usize] {
                            return Err(format!("{:?} defined twice", v));
                        }
                        self.op(*v, op)?;
                        self.ever[v.0 as usize] = true;
                        self.visible[v.0 as usize] = true;
                        self.scope.push(v.0);
                    }
                    Stmt::Set(var, v) => {
                        let want = self.var_ty(*var)?;
                        self.used(*v, Some(want))?;
                    }
                    Stmt::Store { region, base, extent, off, val } => {
                        if self.used(*val, None)? == Ty::F64 {
                            return Err("memory words are 32-bit: an f64 is stored as two words".into());
                        }
                        self.access(*region, *base, *extent, *off, true)?;
                    }
                    Stmt::Out { idx, val, .. } => {
                        if !self.r.io {
                            return Err("audio output in a program without audio I/O".into());
                        }
                        self.used(*idx, Some(Ty::I32))?;
                        self.used(*val, Some(Ty::F32))?;
                    }
                    Stmt::If(c, t, e) => {
                        self.used(*c, Some(Ty::Bool))?;
                        self.block(t, depth)?;
                        self.block(e, depth)?;
                    }
                    Stmt::Loop { body, .. } => self.block(body, depth + 1)?,
                    Stmt::CallHost { f, args, slices, rets } => {
                        if self.r.io {
                            return Err("a host call in an audio program".into());
                        }
                        let Some(h) = crate::host::get(*f) else {
                            return Err(format!("no host function {}", f));
                        };
                        if args.len() != h.params.len() || slices.len() != h.slices.len() || rets.len() != h.rets.len() {
                            return Err(format!("host call `{}`: wrong arity", h.name));
                        }
                        for (v, t) in args.iter().zip(h.params) {
                            self.used(*v, Some(*t))?;
                        }
                        for (x, sig) in slices.iter().zip(h.slices) {
                            self.used(x.off, Some(Ty::I32))?;
                            self.used(x.len, Some(Ty::I32))?;
                            let Some(w) = self.r.bufs.get(x.buf as usize) else {
                                return Err(format!("host call `{}`: no host buffer {}", h.name, x.buf));
                            };
                            if sig.writable && !*w {
                                return Err(format!("host call `{}` writes read-only buffer {}", h.name, x.buf));
                            }
                        }
                        for (v, t) in rets.iter().zip(h.rets) {
                            if self.ty(*v)? != *t {
                                return Err(format!("host call `{}`: result {:?} is not {:?}", h.name, v, t));
                            }
                            if self.ever[v.0 as usize] {
                                return Err(format!("{:?} defined twice", v));
                            }
                            self.ever[v.0 as usize] = true;
                            self.visible[v.0 as usize] = true;
                            self.scope.push(v.0);
                        }
                    }
                    Stmt::Break(d) | Stmt::Continue(d) => {
                        if *d >= depth {
                            return Err("break/continue outside its loop".into());
                        }
                    }
                    Stmt::Call { f, args, rets } => {
                        let Some(g) = self.funcs.get(*f as usize) else {
                            return Err(format!("no function {}", f));
                        };
                        if args.len() != g.params.len() || rets.len() != g.results.len() {
                            return Err(format!("call of function {}: wrong arity", f));
                        }
                        for (a, pv) in args.iter().zip(&g.params) {
                            let want = *g.vals.get(pv.0 as usize).ok_or("a parameter out of range")?;
                            self.used(*a, Some(want))?;
                        }
                        for (v, r) in rets.iter().zip(&g.results) {
                            let want = *g.vals.get(r.0 as usize).ok_or("a result out of range")?;
                            if self.ty(*v)? != want {
                                return Err(format!("call of function {}: result {:?} is not {:?}", f, v, want));
                            }
                            if self.ever[v.0 as usize] {
                                return Err(format!("{:?} defined twice", v));
                            }
                            self.ever[v.0 as usize] = true;
                            self.visible[v.0 as usize] = true;
                            self.scope.push(v.0);
                        }
                    }
                }
            }
            // Values defined in this block are not visible after it.
            for v in self.scope.drain(mark..) {
                self.visible[v as usize] = false;
            }
            Ok(())
        }
    }
    // Functions: none calls itself through any chain (calls nest finitely).
    fn calls(b: &Block, out: &mut Vec<u16>) {
        for s in b {
            match s {
                Stmt::Call { f, .. } => out.push(*f),
                Stmt::If(_, t, e) => {
                    calls(t, out);
                    calls(e, out);
                }
                Stmt::Loop { body, .. } => calls(body, out),
                _ => {}
            }
        }
    }
    let edges: Vec<Vec<u16>> = p
        .funcs
        .iter()
        .map(|g| {
            let mut out = Vec::new();
            calls(&g.body, &mut out);
            out
        })
        .collect();
    // 0 unvisited, 1 on the path, 2 done.
    fn acyclic(k: usize, edges: &[Vec<u16>], state: &mut [u8]) -> bool {
        match state[k] {
            1 => return false,
            2 => return true,
            _ => {}
        }
        state[k] = 1;
        for &g in &edges[k] {
            if (g as usize) < edges.len() && !acyclic(g as usize, edges, state) {
                return false;
            }
        }
        state[k] = 2;
        true
    }
    let mut state = vec![0u8; edges.len()];
    if !(0..edges.len()).all(|k| acyclic(k, &edges, &mut state)) {
        return Err("functions call each other in a cycle".into());
    }
    for (k, g) in p.funcs.iter().enumerate() {
        if !g.funcs.is_empty() {
            return Err(format!("function {} has functions of its own", k));
        }
        let mut v = V { p: g, r: regions, visible: vec![false; g.vals.len()], ever: vec![false; g.vals.len()], scope: Vec::new(), funcs: &p.funcs };
        for pv in &g.params {
            if pv.0 as usize >= g.vals.len() || v.ever[pv.0 as usize] {
                return Err(format!("function {}: bad parameter {:?}", k, pv));
            }
            v.ever[pv.0 as usize] = true;
            v.visible[pv.0 as usize] = true;
        }
        v.block(&g.body, 0).map_err(|e| format!("function {}: {}", k, e))?;
        // Results: parameters or values the top-level block defines.
        let top: std::collections::HashSet<u32> = g
            .body
            .iter()
            .flat_map(|s| match s {
                Stmt::Def(d, _) => vec![d.0],
                Stmt::Call { rets, .. } | Stmt::CallHost { rets, .. } => rets.iter().map(|r| r.0).collect(),
                _ => vec![],
            })
            .chain(g.params.iter().map(|x| x.0))
            .collect();
        if let Some(r) = g.results.iter().find(|r| !top.contains(&r.0)) {
            return Err(format!("function {}: result {:?} is not defined at its top level", k, r));
        }
    }
    let mut v = V { p, r: regions, visible: vec![false; p.vals.len()], ever: vec![false; p.vals.len()], scope: Vec::new(), funcs: &p.funcs };
    // The top block's values stay visible to the end; nothing follows it.
    v.block(&p.body, 0)
}

/// Operand and result types of a unary op.
pub fn un_types(u: Un) -> (Ty, Ty) {
    use Ty::*;
    match u {
        Un::NegF | Un::AbsF | Un::SqrtF | Un::FloorF | Un::CeilF | Un::TruncF | Un::RoundF => (F32, F32),
        Un::F2I | Un::BitsFI => (F32, I32),
        Un::I2F | Un::BitsIF => (I32, F32),
        Un::NegI => (I32, I32),
        Un::NotB => (Bool, Bool),
        Un::NegD | Un::AbsD | Un::SqrtD | Un::FloorD | Un::CeilD | Un::TruncD | Un::RoundD => (F64, F64),
        Un::F2D => (F32, F64),
        Un::D2F => (F64, F32),
        Un::D2I | Un::HiD | Un::LoD => (F64, I32),
        Un::I2D => (I32, F64),
    }
}

/// Operand and result types of a binary op.
pub fn bin_types(b: Bin) -> (Ty, Ty) {
    use Ty::*;
    match b {
        Bin::AddF | Bin::SubF | Bin::MulF | Bin::DivF | Bin::MinF | Bin::MaxF => (F32, F32),
        Bin::AddD | Bin::SubD | Bin::MulD | Bin::DivD | Bin::MinD | Bin::MaxD => (F64, F64),
        Bin::MakeD => (I32, F64),
        Bin::AndB | Bin::OrB => (Bool, Bool),
        _ => (I32, I32),
    }
}

/// The operands an op reads.
pub fn op_uses(op: &Op) -> Vec<Val> {
    match *op {
        Op::ConstF(_) | Op::ConstD(_) | Op::ConstI(_) | Op::ConstB(_) | Op::Get(_) | Op::FrameCount | Op::BufLen(_) => vec![],
        Op::Un(_, a) | Op::Wrap(a, _) => vec![a],
        Op::Bin(_, a, b) | Op::CmpF(_, a, b) | Op::CmpD(_, a, b) | Op::CmpI(_, a, b) => vec![a, b],
        Op::Sel(c, a, b) | Op::Fma(_, a, b, c) => vec![c, a, b],
        Op::Load { off, .. } => off.into_iter().collect(),
        Op::In { idx, .. } => vec![idx],
    }
}

/// The result type an op implies (None: taken from its declaration).
pub fn op_ty(p: &Program, op: &Op) -> Option<Ty> {
    Some(match *op {
        Op::ConstF(_) => Ty::F32,
        Op::ConstD(_) => Ty::F64,
        Op::ConstI(_) | Op::Wrap(..) | Op::FrameCount | Op::BufLen(_) => Ty::I32,
        Op::ConstB(_) | Op::CmpF(..) | Op::CmpD(..) | Op::CmpI(..) => Ty::Bool,
        Op::Get(v) => p.vars[v.0 as usize],
        Op::Un(u, _) => un_types(u).1,
        Op::Bin(b, _, _) => bin_types(b).1,
        Op::Sel(_, a, _) => p.vals[a.0 as usize],
        Op::Fma(Fma::MulAddI, ..) => Ty::I32,
        Op::In { .. } | Op::Fma(..) => Ty::F32,
        Op::Load { .. } => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn magic_division_is_exact() {
        let mut r = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            r
        };
        let mut ds: Vec<u32> = (2..2000).collect();
        ds.extend([1505, 140049, 28476, 1582, 0x7FFF_FFFF, 0x4000_0001, 3, 7, 641, 65537]);
        for _ in 0..2000 {
            ds.push(2 + (next() as u32 % 0x7FFF_FFFE));
        }
        let edge = [0, 1, -1, i32::MAX, i32::MIN, i32::MIN + 1, i32::MAX - 1];
        for d in ds {
            for k in 0..300 {
                let x = if k < edge.len() { edge[k] } else { next() as u32 as i32 };
                for x in [x, x / 1000, x % 5000] {
                    assert_eq!(super::div_by_magic(x, d), x.wrapping_div(d as i32), "{} / {}", x, d);
                }
            }
        }
    }
}

// =========================================================================
// Calls flattened
// =========================================================================

/// `p` flattened ([`inline_calls`]) and optimized again (every pass keeps
/// values: the same bits as the program with its calls): what the native
/// backends compile. Borrowed when `p` has no functions.
pub fn flat(p: &Program) -> std::borrow::Cow<'_, Program> {
    if p.funcs.is_empty() {
        return std::borrow::Cow::Borrowed(p);
    }
    let mut q = inline_calls(p);
    crate::opt::optimize(&mut q);
    std::borrow::Cow::Owned(q)
}

/// `p` with every [`Stmt::Call`] replaced by its function's body (fresh
/// values and variables, the params read as the arguments, the call's
/// results as the function's): the same computation, for backends that
/// compile one straight body (native code; the optimizer may then run on
/// it, every pass keeping values).
pub fn inline_calls(p: &Program) -> Program {
    if p.funcs.is_empty() {
        return p.clone();
    }
    let mut out = Program { vals: p.vals.clone(), vars: p.vars.clone(), frame_words: p.frame_words, ..Default::default() };
    let mut vmap: Vec<Val> = (0..p.vals.len() as u32).map(Val).collect();
    let rmap: Vec<Var> = (0..p.vars.len() as u32).map(Var).collect();
    out.body = inline_block(&p.body, &mut out, &p.funcs, &mut vmap, &rmap);
    out
}

fn inline_block(b: &Block, out: &mut Program, funcs: &[Program], vmap: &mut Vec<Val>, rmap: &[Var]) -> Block {
    let mut res = Vec::with_capacity(b.len());
    for s in b {
        match s {
            Stmt::Call { f, args, rets } => {
                let g = &funcs[*f as usize];
                // Fresh values and variables for this copy.
                let mut gmap: Vec<Val> = g
                    .vals
                    .iter()
                    .map(|t| {
                        out.vals.push(*t);
                        Val(out.vals.len() as u32 - 1)
                    })
                    .collect();
                for (pv, a) in g.params.iter().zip(args) {
                    gmap[pv.0 as usize] = vmap[a.0 as usize];
                }
                let gr: Vec<Var> = g
                    .vars
                    .iter()
                    .map(|t| {
                        out.vars.push(*t);
                        Var(out.vars.len() as u32 - 1)
                    })
                    .collect();
                res.extend(inline_block(&g.body, out, funcs, &mut gmap, &gr));
                // Later reads of the call's results read the function's.
                for (r, x) in rets.iter().zip(&g.results) {
                    vmap[r.0 as usize] = gmap[x.0 as usize];
                }
            }
            s => res.push(map_stmt(s, out, funcs, vmap, rmap)),
        }
    }
    res
}

fn map_stmt(s: &Stmt, out: &mut Program, funcs: &[Program], vmap: &mut Vec<Val>, rmap: &[Var]) -> Stmt {
    let v = |x: &Val, vmap: &Vec<Val>| vmap[x.0 as usize];
    match s {
        Stmt::Def(d, op) => Stmt::Def(v(d, vmap), map_op(op, vmap, rmap)),
        Stmt::Set(r, x) => Stmt::Set(rmap[r.0 as usize], v(x, vmap)),
        Stmt::Store { region, base, extent, off, val } => Stmt::Store { region: *region, base: *base, extent: *extent, off: off.map(|o| v(&o, vmap)), val: v(val, vmap) },
        Stmt::Out { ch, idx, val } => Stmt::Out { ch: *ch, idx: v(idx, vmap), val: v(val, vmap) },
        Stmt::If(c, t, e) => {
            let c = v(c, vmap);
            Stmt::If(c, inline_block(t, out, funcs, vmap, rmap), inline_block(e, out, funcs, vmap, rmap))
        }
        Stmt::Loop { cap, body } => Stmt::Loop { cap: *cap, body: inline_block(body, out, funcs, vmap, rmap) },
        Stmt::Break(d) => Stmt::Break(*d),
        Stmt::Continue(d) => Stmt::Continue(*d),
        Stmt::CallHost { f, args, slices, rets } => Stmt::CallHost {
            f: *f,
            args: args.iter().map(|a| v(a, vmap)).collect(),
            slices: slices.iter().map(|x| SliceArg { buf: x.buf, off: v(&x.off, vmap), len: v(&x.len, vmap) }).collect(),
            rets: rets.iter().map(|r| v(r, vmap)).collect(),
        },
        Stmt::Call { .. } => unreachable!("inlined by inline_block"),
    }
}

/// `op` with its values renamed by `vmap` and variables by `rmap`.
pub fn map_op(op: &Op, vmap: &[Val], rmap: &[Var]) -> Op {
    let v = |x: Val| vmap[x.0 as usize];
    match *op {
        Op::Get(r) => Op::Get(rmap[r.0 as usize]),
        Op::Un(u, a) => Op::Un(u, v(a)),
        Op::Bin(b, x, y) => Op::Bin(b, v(x), v(y)),
        Op::CmpF(c, x, y) => Op::CmpF(c, v(x), v(y)),
        Op::CmpI(c, x, y) => Op::CmpI(c, v(x), v(y)),
        Op::CmpD(c, x, y) => Op::CmpD(c, v(x), v(y)),
        Op::Sel(c, x, y) => Op::Sel(v(c), v(x), v(y)),
        Op::Fma(k, a, b, c) => Op::Fma(k, v(a), v(b), v(c)),
        Op::Wrap(x, n) => Op::Wrap(v(x), n),
        Op::Load { region, base, extent, off } => Op::Load { region, base, extent, off: off.map(v) },
        Op::In { ch, idx } => Op::In { ch, idx: v(idx) },
        op => op,
    }
}
