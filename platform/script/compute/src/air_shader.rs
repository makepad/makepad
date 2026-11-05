//! A kernel's AIR (flattened, checked against what a shader can run) as a shader function's
//! body, inside the compiler. Every AIR op maps to the shader op with the same IEEE meaning
//! (min/max as selects, integer ops wrapping, saturating conversions written out).
//!
//! What the kernel's memory is on the GPU is the route's ([`Io`]): a host's point route
//! ([`crate::shader_route`]) and compute programs' element route ([`crate::element_route`])
//! each own theirs, so either one deletes alone.
//!
//! SHADER LAW: unary minus has the lowest precedence in the script shader
//! parser; write `(0.0 - a) - b`, never `-a - b`.
use crate::ir::{self as air, Bin, Cmp, Op, Region, Ty, Un};
use makepad_script::shader_ir::*;
use makepad_script::*;

/// A route's kernel memory: its context words and host buffers as the shader holds them.
pub(crate) trait Io {
    /// Context word `base` (a constant place), as a value of `ty`.
    fn ctx(&self, t: &mut Tr, base: u32, ty: TyId) -> Result<ExprId, String>;
    /// Host buffer `b`'s word `base + off` as a value of `ty`; None: the route has no such
    /// buffer to read.
    fn load(&self, t: &mut Tr, b: u8, base: u32, off: Option<air::Val>, ty: TyId) -> Result<Option<ExprId>, String>;
    /// `val` written to host buffer `b`'s word `base + off`; None: the route has no such buffer
    /// to write.
    fn store(&self, t: &mut Tr, b: u8, base: u32, off: Option<air::Val>, val: air::Val) -> Result<Option<Stmt>, String>;
    /// Host buffer `b`'s length in words, as i32.
    fn len(&self, t: &mut Tr, b: u8) -> Result<ExprId, String>;
    /// Whether fast math's fused multiply-adds run (as the GPU's fused multiply-add).
    fn fma(&self) -> bool;
}

/// Whether a block holds a `Break(d)`/`Continue(d)` past its own loop.
fn has_deep_jump(b: &air::Block) -> bool {
    b.iter().any(|s| match s {
        air::Stmt::Break(d) | air::Stmt::Continue(d) => *d > 0,
        air::Stmt::If(_, t, e) => has_deep_jump(t) || has_deep_jump(e),
        air::Stmt::Loop { body, .. } => has_deep_jump(body),
        _ => false,
    })
}

pub(crate) fn ir_ty(t: Ty) -> Result<TyId, String> {
    match t {
        Ty::F32 => Ok(TY_F32),
        Ty::I32 => Ok(TY_I32),
        Ty::Bool => Ok(TY_BOOL),
        Ty::F64 => Err("f64 arithmetic (no GPU route)".into()),
    }
}

/// One translation: AIR values and variables as shader locals.
pub(crate) struct Tr<'a> {
    program: &'a air::Program,
    /// The compiler's control word buffer (the cancel token): reads 0.
    control: Option<u8>,
    pub f: IrFunction,
    vals: Vec<LocalId>,
    vars: Vec<LocalId>,
    frame: Option<LocalId>,
    /// Multi-level breaks: 2 d (+1 for a continue) of the loop to leave.
    jump: Option<LocalId>,
}

impl<'a> Tr<'a> {
    /// The translation of `program` into `f`; `control` is its control word buffer.
    pub fn new(program: &'a air::Program, control: Option<u8>, f: IrFunction) -> Tr<'a> {
        Tr { program, control, f, vals: Vec::new(), vars: Vec::new(), frame: None, jump: None }
    }
    pub fn e(&mut self, kind: ExprKind, ty: TyId) -> ExprId {
        self.f.expr(kind, ty)
    }
    pub fn local(&mut self, l: LocalId, ty: TyId) -> ExprId {
        self.e(ExprKind::Local(l), ty)
    }
    pub fn lit_f(&mut self, x: f32) -> ExprId {
        if x == 0.0 || x.is_normal() {
            self.e(ExprKind::Lit(IrLit::F32(x)), TY_F32)
        } else {
            // inf, nan and subnormals by their bits (the text forms are not exact)
            let b = self.lit_i(x.to_bits() as i32);
            self.e(ExprKind::Bitcast(b), TY_F32)
        }
    }
    pub fn lit_i(&mut self, x: i32) -> ExprId {
        self.e(ExprKind::Lit(IrLit::I32(x)), TY_I32)
    }
    pub fn lit_u(&mut self, x: u32) -> ExprId {
        self.e(ExprKind::Lit(IrLit::U32(x)), TY_U32)
    }
    fn lit_b(&mut self, x: bool) -> ExprId {
        self.e(ExprKind::Lit(IrLit::Bool(x)), TY_BOOL)
    }
    pub fn bin(&mut self, op: BinOp, a: ExprId, b: ExprId, ty: TyId) -> ExprId {
        self.e(ExprKind::Binary(op, a, b), ty)
    }
    pub fn sel(&mut self, c: ExprId, a: ExprId, b: ExprId, ty: TyId) -> ExprId {
        self.e(ExprKind::Select(c, a, b), ty)
    }
    fn call(&mut self, op: Builtin, args: Vec<ExprId>, ty: TyId) -> ExprId {
        self.e(ExprKind::Builtin(op, args), ty)
    }
    pub fn as_u(&mut self, a: ExprId) -> ExprId {
        self.e(ExprKind::Bitcast(a), TY_U32)
    }
    pub fn as_i(&mut self, a: ExprId) -> ExprId {
        self.e(ExprKind::Bitcast(a), TY_I32)
    }
    /// An i32 op done on u32 bits (wrapping, never signed overflow).
    fn wrapping(&mut self, op: BinOp, a: ExprId, b: ExprId) -> ExprId {
        let (ua, ub) = (self.as_u(a), self.as_u(b));
        let r = self.bin(op, ua, ub, TY_U32);
        self.as_i(r)
    }
    pub fn val(&mut self, v: air::Val) -> ExprId {
        let ty = ir_ty(self.program.vals[v.0 as usize]).unwrap_or(TY_F32);
        self.local(self.vals[v.0 as usize], ty)
    }
    /// The AIR type of value `v`.
    pub fn val_ty(&self, v: air::Val) -> Ty {
        self.program.vals[v.0 as usize]
    }
    /// A local of `ty`, zeroed.
    pub fn declare(&mut self, ty: Ty) -> Result<(LocalId, Stmt), String> {
        let t = ir_ty(ty)?;
        let zero = match ty {
            Ty::F32 => self.lit_f(0.0),
            Ty::I32 => self.lit_i(0),
            _ => self.lit_b(false),
        };
        let l = self.f.local(id!(k), 0, t, true, LocalKind::Temp);
        Ok((l, Stmt::Local(l, zero)))
    }
    /// A word as a value of type `ty` (`w` is an i32 or f32 expression of the word's bits).
    pub fn word_as(&mut self, w: ExprId, w_is_f: bool, ty: TyId) -> ExprId {
        match (ty, w_is_f) {
            (TY_F32, true) | (TY_I32, false) => w,
            (TY_F32, false) => self.e(ExprKind::Bitcast(w), TY_F32),
            (TY_I32, true) => self.as_i(w),
            _ => {
                let wi = if w_is_f { self.as_i(w) } else { w };
                let z = self.lit_i(0);
                self.bin(BinOp::Ne, wi, z, TY_BOOL)
            }
        }
    }
    /// A value as the i32 bits a memory word holds.
    pub fn to_word(&mut self, v: air::Val) -> ExprId {
        let ty = self.program.vals[v.0 as usize];
        let e = self.val(v);
        match ty {
            Ty::F32 => self.as_i(e),
            Ty::Bool => {
                let (one, zero) = (self.lit_i(1), self.lit_i(0));
                self.sel(e, one, zero, TY_I32)
            }
            _ => e,
        }
    }
    /// `base + clamp(off, 0, extent - 1)` (an offset read as u32), as i32.
    fn index(&mut self, base: u32, extent: u32, off: Option<air::Val>) -> ExprId {
        match off {
            None => self.lit_i(base as i32),
            Some(o) => {
                let o = self.val(o);
                let ou = self.as_u(o);
                let hi = self.lit_u(extent.saturating_sub(1));
                let over = self.bin(BinOp::Gt, ou, hi, TY_BOOL);
                let hi2 = self.lit_u(extent.saturating_sub(1));
                let c = self.sel(over, hi2, ou, TY_U32);
                let b = self.lit_u(base);
                let s = self.bin(BinOp::Add, c, b, TY_U32);
                self.as_i(s)
            }
        }
    }
    /// The program's values, variables and frame, declared.
    pub fn locals(&mut self, body: &mut Vec<Stmt>, frame_ty: Option<TyId>) -> Result<(), String> {
        for ty in self.program.vals.clone() {
            let (l, s) = self.declare(ty)?;
            self.vals.push(l);
            body.push(s);
        }
        for ty in self.program.vars.clone() {
            let (l, s) = self.declare(ty)?;
            self.vars.push(l);
            body.push(s);
        }
        if let Some(fty) = frame_ty {
            let zero = self.lit_i(0);
            let items = vec![zero; self.program.frame_words as usize];
            let init = self.f.expr(ExprKind::Construct(items), fty);
            let l = self.f.local(id!(frame), 0, fty, true, LocalKind::Temp);
            body.push(Stmt::Local(l, init));
            self.frame = Some(l);
        }
        Ok(())
    }
    /// The jump word multi-level breaks need, declared when the program has one.
    pub fn jumps(&mut self, body: &mut Vec<Stmt>) -> Result<(), String> {
        if has_deep_jump(&self.program.body) {
            let (j, s) = self.declare(Ty::I32)?;
            self.jump = Some(j);
            body.push(s);
        }
        Ok(())
    }
    /// Signed division by a divisor known to be neither 0 nor -1.
    fn safe_div(&mut self, a: ExprId, b: ExprId) -> (ExprId, ExprId, ExprId) {
        let (z, m1, one) = (self.lit_i(0), self.lit_i(-1), self.lit_i(1));
        let bz = self.bin(BinOp::Eq, b, z, TY_BOOL);
        let bm = self.bin(BinOp::Eq, b, m1, TY_BOOL);
        let bad = self.bin(BinOp::LogicOr, bz, bm, TY_BOOL);
        let sb = self.sel(bad, one, b, TY_I32);
        let q = self.bin(BinOp::Div, a, sb, TY_I32);
        (q, sb, bz)
    }

    fn op(&mut self, io: &dyn Io, op: &Op, ty: TyId) -> Result<ExprId, String> {
        let program = self.program;
        Ok(match *op {
            Op::ConstF(x) => self.lit_f(x),
            Op::ConstI(x) => self.lit_i(x),
            Op::ConstB(x) => self.lit_b(x),
            Op::ConstD(_) => return Err("f64 constant".into()),
            Op::Get(v) => {
                let t = ir_ty(program.vars[v.0 as usize])?;
                self.local(self.vars[v.0 as usize], t)
            }
            Op::Un(u, a) => {
                let x = self.val(a);
                match u {
                    Un::NegF => self.e(ExprKind::Unary(UnOp::Neg, x), TY_F32),
                    Un::AbsF => self.call(Builtin::Abs, vec![x], TY_F32),
                    Un::SqrtF => self.call(Builtin::Sqrt, vec![x], TY_F32),
                    Un::FloorF => self.call(Builtin::Floor, vec![x], TY_F32),
                    Un::CeilF => self.call(Builtin::Ceil, vec![x], TY_F32),
                    Un::TruncF => self.call(Builtin::Trunc, vec![x], TY_F32),
                    Un::RoundF => {
                        // half away from zero: trunc, then a step where the fraction is >= 0.5
                        let t = self.call(Builtin::Trunc, vec![x], TY_F32);
                        let d = self.bin(BinOp::Sub, x, t, TY_F32);
                        let ad = self.call(Builtin::Abs, vec![d], TY_F32);
                        let h = self.lit_f(0.5);
                        let up = self.bin(BinOp::Ge, ad, h, TY_BOOL);
                        let z = self.lit_f(0.0);
                        let neg = self.bin(BinOp::Lt, x, z, TY_BOOL);
                        let (m1, p1) = (self.lit_f(-1.0), self.lit_f(1.0));
                        let s = self.sel(neg, m1, p1, TY_F32);
                        let ts = self.bin(BinOp::Add, t, s, TY_F32);
                        self.sel(up, ts, t, TY_F32)
                    }
                    Un::F2I => {
                        // truncating, saturating, NaN -> 0 (Rust `as`)
                        let c = self.e(ExprKind::Convert(x), TY_I32);
                        let two31 = self.lit_f(2147483648.0);
                        let hi = self.bin(BinOp::Ge, x, two31, TY_BOOL);
                        let max = self.lit_i(i32::MAX);
                        let r = self.sel(hi, max, c, TY_I32);
                        let m31 = self.lit_f(-2147483648.0);
                        let lo = self.bin(BinOp::Lt, x, m31, TY_BOOL);
                        let min = self.lit_i(i32::MIN);
                        let r = self.sel(lo, min, r, TY_I32);
                        let nan = self.bin(BinOp::Ne, x, x, TY_BOOL);
                        let z = self.lit_i(0);
                        self.sel(nan, z, r, TY_I32)
                    }
                    Un::I2F => self.e(ExprKind::Convert(x), TY_F32),
                    Un::BitsFI => self.as_i(x),
                    Un::BitsIF => self.e(ExprKind::Bitcast(x), TY_F32),
                    Un::NegI => {
                        let z = self.lit_i(0);
                        self.wrapping(BinOp::Sub, z, x)
                    }
                    Un::NotB => self.e(ExprKind::Unary(UnOp::Not, x), TY_BOOL),
                    _ => return Err(format!("f64 op {u:?}")),
                }
            }
            Op::Bin(b, x, y) => {
                let (a, c) = (self.val(x), self.val(y));
                match b {
                    Bin::AddF => self.bin(BinOp::Add, a, c, TY_F32),
                    Bin::SubF => self.bin(BinOp::Sub, a, c, TY_F32),
                    Bin::MulF => self.bin(BinOp::Mul, a, c, TY_F32),
                    Bin::DivF => self.bin(BinOp::Div, a, c, TY_F32),
                    Bin::MinF => {
                        let lt = self.bin(BinOp::Lt, a, c, TY_BOOL);
                        self.sel(lt, a, c, TY_F32)
                    }
                    Bin::MaxF => {
                        let gt = self.bin(BinOp::Gt, a, c, TY_BOOL);
                        self.sel(gt, a, c, TY_F32)
                    }
                    Bin::AddI => self.wrapping(BinOp::Add, a, c),
                    Bin::SubI => self.wrapping(BinOp::Sub, a, c),
                    Bin::MulI => self.wrapping(BinOp::Mul, a, c),
                    Bin::DivI => {
                        let (q, _, bz) = self.safe_div(a, c);
                        let m1 = self.lit_i(-1);
                        let bm = self.bin(BinOp::Eq, c, m1, TY_BOOL);
                        let z = self.lit_i(0);
                        let na = self.wrapping(BinOp::Sub, z, a);
                        let r = self.sel(bm, na, q, TY_I32);
                        let z2 = self.lit_i(0);
                        self.sel(bz, z2, r, TY_I32)
                    }
                    Bin::RemI => {
                        let (q, sb, bz) = self.safe_div(a, c);
                        let qb = self.bin(BinOp::Mul, q, sb, TY_I32);
                        let r = self.bin(BinOp::Sub, a, qb, TY_I32);
                        let m1 = self.lit_i(-1);
                        let bm = self.bin(BinOp::Eq, c, m1, TY_BOOL);
                        let z = self.lit_i(0);
                        let r = self.sel(bm, z, r, TY_I32);
                        self.sel(bz, a, r, TY_I32)
                    }
                    Bin::AndI => self.bin(BinOp::BitAnd, a, c, TY_I32),
                    Bin::OrI => self.bin(BinOp::BitOr, a, c, TY_I32),
                    Bin::XorI => self.bin(BinOp::BitXor, a, c, TY_I32),
                    Bin::ShlI | Bin::ShrUI => {
                        let ua = self.as_u(a);
                        let uc = self.as_u(c);
                        let m = self.lit_u(31);
                        let s = self.bin(BinOp::BitAnd, uc, m, TY_U32);
                        let op = if b == Bin::ShlI { BinOp::Shl } else { BinOp::Shr };
                        let r = self.bin(op, ua, s, TY_U32);
                        self.as_i(r)
                    }
                    Bin::ShrI => {
                        let m = self.lit_i(31);
                        let s = self.bin(BinOp::BitAnd, c, m, TY_I32);
                        self.bin(BinOp::Shr, a, s, TY_I32)
                    }
                    Bin::AndB => self.bin(BinOp::LogicAnd, a, c, TY_BOOL),
                    Bin::OrB => self.bin(BinOp::LogicOr, a, c, TY_BOOL),
                    _ => return Err(format!("f64 op {b:?}")),
                }
            }
            Op::CmpF(cc, x, y) | Op::CmpI(cc, x, y) => {
                let (a, c) = (self.val(x), self.val(y));
                let op = match cc {
                    Cmp::Lt => BinOp::Lt,
                    Cmp::Le => BinOp::Le,
                    Cmp::Gt => BinOp::Gt,
                    Cmp::Ge => BinOp::Ge,
                    Cmp::Eq => BinOp::Eq,
                    Cmp::Ne => BinOp::Ne,
                };
                self.bin(op, a, c, TY_BOOL)
            }
            Op::CmpD(..) => return Err("f64 compare".into()),
            Op::Sel(c, x, y) => {
                let (cc, a, b) = (self.val(c), self.val(x), self.val(y));
                self.sel(cc, a, b, ty)
            }
            // the exact integer multiply-add every kernel gets: a wrapping product and sum
            Op::Fma(air::Fma::MulAddI, a, b, c) => {
                let (x, y, z) = (self.val(a), self.val(b), self.val(c));
                let m = self.wrapping(BinOp::Mul, x, y);
                self.wrapping(BinOp::Add, m, z)
            }
            // fast math where the route runs it: the GPU's fused multiply-add (the same on each
            // device class; the CPU's fuses too)
            Op::Fma(kind, a, b, c) if io.fma() => {
                let (x, y, z) = (self.val(a), self.val(b), self.val(c));
                let (x, z) = match kind {
                    air::Fma::Add => (x, z),
                    air::Fma::SubFrom => (self.e(ExprKind::Unary(UnOp::Neg, x), TY_F32), z),
                    air::Fma::Sub => (x, self.e(ExprKind::Unary(UnOp::Neg, z), TY_F32)),
                    air::Fma::MulAddI => unreachable!("matched above"),
                };
                self.call(Builtin::Fma, vec![x, y, z], TY_F32)
            }
            Op::Fma(..) => return Err("fused multiply-add (fast math)".into()),
            Op::Wrap(x, len) => {
                let a = self.val(x);
                if len.is_power_of_two() {
                    let m = self.lit_i(len as i32 - 1);
                    self.bin(BinOp::BitAnd, a, m, TY_I32)
                } else {
                    let l = self.lit_i(len as i32);
                    let q = self.bin(BinOp::Div, a, l, TY_I32);
                    let l2 = self.lit_i(len as i32);
                    let ql = self.bin(BinOp::Mul, q, l2, TY_I32);
                    let r = self.bin(BinOp::Sub, a, ql, TY_I32);
                    let z = self.lit_i(0);
                    let neg = self.bin(BinOp::Lt, r, z, TY_BOOL);
                    let l3 = self.lit_i(len as i32);
                    let rl = self.bin(BinOp::Add, r, l3, TY_I32);
                    self.sel(neg, rl, r, TY_I32)
                }
            }
            Op::Load { region: Region::Ctx, base, off, .. } => {
                if off.is_some() {
                    return Err("a computed context read".into());
                }
                io.ctx(self, base, ty)?
            }
            Op::Load { region: Region::Buf(b), .. } if Some(b) == self.control => {
                // never cancelled
                let z = self.lit_i(0);
                self.word_as(z, false, ty)
            }
            Op::Load { region: Region::Frame, base, extent, off } => {
                let Some(fr) = self.frame else { return Err("a frame read without a frame".into()) };
                let fty = self.f.locals[fr as usize].ty;
                let arr = self.local(fr, fty);
                let i = self.index(base, extent, off);
                let w = self.e(ExprKind::Index(arr, i), TY_I32);
                self.word_as(w, false, ty)
            }
            Op::Load { region, base, off, .. } => {
                let read = match region {
                    Region::Buf(b) => io.load(self, b, base, off, ty)?,
                    _ => None,
                };
                let Some(read) = read else { return Err(format!("a read of {region:?} memory")) };
                read
            }
            Op::In { .. } => return Err("an audio input".into()),
            Op::FrameCount => self.lit_i(1),
            Op::BufLen(b) => io.len(self, b)?,
        })
    }

    pub fn block(&mut self, io: &dyn Io, b: &air::Block, depth: u32) -> Result<Vec<Stmt>, String> {
        let program = self.program;
        let mut out = Vec::new();
        for s in b {
            match s {
                air::Stmt::Def(v, op) => {
                    let ty = ir_ty(program.vals[v.0 as usize])?;
                    let e = self.op(io, op, ty)?;
                    let l = self.local(self.vals[v.0 as usize], ty);
                    out.push(Stmt::Assign(l, e));
                }
                air::Stmt::Set(var, v) => {
                    let ty = ir_ty(program.vars[var.0 as usize])?;
                    let e = self.val(*v);
                    let l = self.local(self.vars[var.0 as usize], ty);
                    out.push(Stmt::Assign(l, e));
                }
                // work counters and flags: nothing reads them back here
                air::Stmt::Store { region: Region::Ctx, .. } => {}
                air::Stmt::Store { region: Region::Frame, base, extent, off, val } => {
                    let Some(fr) = self.frame else { return Err("a frame write without a frame".into()) };
                    let fty = self.f.locals[fr as usize].ty;
                    let arr = self.local(fr, fty);
                    let i = self.index(*base, *extent, *off);
                    let place = self.e(ExprKind::Index(arr, i), TY_I32);
                    let w = self.to_word(*val);
                    out.push(Stmt::Assign(place, w));
                }
                air::Stmt::Store { region, base, off, val, .. } => {
                    let write = match *region {
                        Region::Buf(b) => io.store(self, b, *base, *off, *val)?,
                        _ => None,
                    };
                    let Some(write) = write else { return Err(format!("a write to {region:?} memory")) };
                    out.push(write);
                }
                air::Stmt::Out { .. } => return Err("an audio output".into()),
                air::Stmt::If(c, t, e) => {
                    let c = self.val(*c);
                    let t = self.block(io, t, depth)?;
                    let e = self.block(io, e, depth)?;
                    out.push(Stmt::If(c, t, e));
                }
                air::Stmt::Loop { cap, body } => {
                    let g = self.f.local(id!(g), 0, TY_U32, true, LocalKind::Guard);
                    let body = self.block(io, body, depth + 1)?;
                    let (s, e) = (self.lit_u(0), self.lit_u(*cap));
                    out.push(Stmt::For(g, s, e, body));
                    // a jump past the loop just left
                    if let (Some(j), true) = (self.jump, depth > 0) {
                        let jl = self.local(j, TY_I32);
                        let z = self.lit_i(0);
                        let pending = self.bin(BinOp::Gt, jl, z, TY_BOOL);
                        let jl2 = self.local(j, TY_I32);
                        let two = self.lit_i(2);
                        let dec = self.bin(BinOp::Sub, jl2, two, TY_I32);
                        let jset = self.local(j, TY_I32);
                        let jr = self.local(j, TY_I32);
                        let z2 = self.lit_i(0);
                        let here_break = self.bin(BinOp::Eq, jr, z2, TY_BOOL);
                        let jr2 = self.local(j, TY_I32);
                        let one = self.lit_i(1);
                        let here_cont = self.bin(BinOp::Eq, jr2, one, TY_BOOL);
                        let jz = self.local(j, TY_I32);
                        let z3 = self.lit_i(0);
                        out.push(Stmt::If(
                            pending,
                            vec![
                                Stmt::Assign(jset, dec),
                                Stmt::If(here_break, vec![Stmt::Break], vec![Stmt::If(here_cont, vec![Stmt::Assign(jz, z3), Stmt::Continue], vec![Stmt::Break])]),
                            ],
                            Vec::new(),
                        ));
                    }
                }
                air::Stmt::Break(d) | air::Stmt::Continue(d) => {
                    let cont = matches!(s, air::Stmt::Continue(_));
                    if *d == 0 {
                        out.push(if cont { Stmt::Continue } else { Stmt::Break });
                    } else {
                        let Some(j) = self.jump else { return Err("a jump past a loop".into()) };
                        let jl = self.local(j, TY_I32);
                        let v = self.lit_i(2 * *d as i32 + cont as i32);
                        out.push(Stmt::Assign(jl, v));
                        out.push(Stmt::Break);
                    }
                }
                air::Stmt::Call { .. } => return Err("a call (not flattened)".into()),
                air::Stmt::CallHost { .. } => return Err("a host call".into()),
            }
        }
        Ok(out)
    }
}
