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

    /// Worst-case ops of one whole run: every loop at its cap (`init()`).
    pub fn total_cost(&self) -> u64 {
        fn walk(b: &Block) -> u64 {
            let mut sum = 0u64;
            for s in b {
                sum = sum.saturating_add(match s {
                    Stmt::If(_, t, e) => 1 + walk(t).max(walk(e)),
                    Stmt::Loop { cap, body } => (*cap as u64).saturating_mul(walk(body) + 2),
                    _ => 1,
                });
            }
            sum
        }
        walk(&self.body)
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
    off.min(extent.saturating_sub(1))
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
            Op::Load { region: Region::Buf(k), base, off, .. } => {
                let Some(b) = self.mem.bufs.get(k as usize) else { return 0 };
                if b.len == 0 {
                    return 0;
                }
                b.load(buf_index(base, off.map(|o| self.r(o)), b.len))
            }
            Op::BufLen(k) => self.mem.bufs.get(k as usize).map_or(0, |b| b.len as u32),
            Op::Load { region, base, extent, off } => {
                let off = match off {
                    Some(o) => clamp_off(self.r(o), extent),
                    None => 0,
                };
                self.read_word(region, base as usize + off as usize)
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
                Stmt::Store { region: Region::Buf(k), base, off, val, .. } => {
                    let x = self.r(*val);
                    let o = off.map(|o| self.r(o));
                    if let Some(b) = self.mem.bufs.get(*k as usize) {
                        if b.len > 0 {
                            b.store(buf_index(*base, o, b.len), x);
                        }
                    }
                }
                Stmt::Store { region, base, extent, off, val } => {
                    let off = match off {
                        Some(o) => clamp_off(self.r(*o), *extent),
                        None => 0,
                    };
                    let x = self.r(*val);
                    self.write_word(*region, *base as usize + off as usize, x);
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
                Op::ConstI(_) => I32,
                Op::ConstB(_) => Bool,
                Op::Get(var) => self.var_ty(var)?,
                Op::Un(u, a) => {
                    let (inp, out) = match u {
                        Un::NegF | Un::AbsF | Un::SqrtF | Un::FloorF | Un::CeilF | Un::TruncF | Un::RoundF => (F32, F32),
                        Un::F2I | Un::BitsFI => (F32, I32),
                        Un::I2F | Un::BitsIF => (I32, F32),
                        Un::NegI => (I32, I32),
                        Un::NotB => (Bool, Bool),
                    };
                    self.used(a, Some(inp))?;
                    out
                }
                Op::Bin(b, x, y) => {
                    let (inp, out) = match b {
                        Bin::AddF | Bin::SubF | Bin::MulF | Bin::DivF | Bin::MinF | Bin::MaxF => (F32, F32),
                        Bin::AndB | Bin::OrB => (Bool, Bool),
                        _ => (I32, I32),
                    };
                    self.used(x, Some(inp))?;
                    self.used(y, Some(inp))?;
                    out
                }
                Op::CmpF(_, x, y) => {
                    self.used(x, Some(F32))?;
                    self.used(y, Some(F32))?;
                    Bool
                }
                Op::CmpI(_, x, y) => {
                    let tx = self.used(x, None)?;
                    self.used(y, Some(tx))?;
                    if tx == F32 {
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
                Op::Wrap(x, len) => {
                    self.used(x, Some(I32))?;
                    if len == 0 {
                        return Err("wrap by 0".into());
                    }
                    I32
                }
                Op::Load { region, base, extent, off } => {
                    self.access(region, base, extent, off, false)?;
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
                        self.used(*val, None)?;
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
                    Stmt::Break(d) | Stmt::Continue(d) => {
                        if *d >= depth {
                            return Err("break/continue outside its loop".into());
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
    let mut v = V { p, r: regions, visible: vec![false; p.vals.len()], ever: vec![false; p.vals.len()], scope: Vec::new() };
    // The top block's values stay visible to the end; nothing follows it.
    v.block(&p.body, 0)
}

/// The operands an op reads.
pub fn op_uses(op: &Op) -> Vec<Val> {
    match *op {
        Op::ConstF(_) | Op::ConstI(_) | Op::ConstB(_) | Op::Get(_) | Op::FrameCount | Op::BufLen(_) => vec![],
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
        Op::ConstI(_) | Op::Wrap(..) | Op::FrameCount | Op::BufLen(_) => Ty::I32,
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
