//! Splash SDF kernels on the GPU.
//!
//! A part's custom distance function (`m.sdf({field: fn(p) {..}})`) is a
//! kernel compiled by the Splash kernel compiler (`makepad_script_compute`)
//! into AIR, the structured IR whose ops are exactly specified. Here that
//! AIR becomes a function of the bake shader, built as shader IR by the
//! shader compiler's host hook ([`ScriptVm::set_shader_ir_fn`]): the program
//! moves as data, never as source text, and every AIR op maps to the shader
//! op with the same IEEE meaning (min/max as selects, integer ops wrapping,
//! saturating conversions written out). The bake shader is `precise_math`
//! (no contraction, correctly rounded division and square root), so a baked
//! brick equals the CPU kernel's distances bit for bit.
//!
//! - The document's numbers the function reads (`vase_height`, ...) are
//!   kernel parameters ([`makepad_csg_sdf::SdfSplashExpr`] lifts them), so a
//!   parameter drag is the same compiled kernel: its values travel in the
//!   program texels (op `KERNEL`) and only the touched bricks bake again.
//! - A volume's kernels have slots ([`KernelSlot`], by program identity); a
//!   bake variant ([`KernelBake`]) holds the dispatch over them. A new
//!   program (a shape-structure change) is a new variant, compiled while the
//!   old bricks keep drawing.
//! - Limits: a kernel's AIR is bounded by construction (every loop has a
//!   cap); one whose worst case exceeds [`MAX_KERNEL_COST`] ops per sample,
//!   reads more than [`MAX_KERNEL_PARAMS`] parameters, uses f64, host calls,
//!   tables or more than [`MAX_FRAME_WORDS`] words of local arrays has no GPU
//!   route: its bricks bake on the CPU twin.
//!
//! SHADER LAW: unary minus has the lowest precedence in the script shader
//! parser; write `(0.0 - a) - b`, never `-a - b`.

use crate::bake::DrawSdfBake;
use crate::march::DrawSdfMarch;
use makepad_csg_sdf::{Sdf3, SdfKernel, SdfNode, SdfSplashExpr};
use makepad_draw::makepad_script::shader_compile::ShaderIrFn;
use makepad_draw::makepad_script::shader_ir::*;
use makepad_draw::makepad_script::trap::NoTrap;
use makepad_draw::*;
use makepad_script_compute::ir::{self as air, Bin, Cmp, Op, Region, Ty, Un};
use std::rc::Rc;
use std::sync::Arc;

/// Worst-case AIR ops of one sample: past it a kernel bakes on the CPU (a
/// bake quad samples 512 points, a march step one; GPU watchdogs kill
/// draws of seconds).
pub const MAX_KERNEL_COST: u64 = 20_000;
/// Parameters a kernel call passes (four texels).
pub const MAX_KERNEL_PARAMS: usize = 16;
/// Words of local arrays (the AIR frame) a GPU kernel may use.
pub const MAX_FRAME_WORDS: u32 = 64;

/// A kernel's AIR made ready for the shader: flattened (no calls), checked
/// against what the GPU route supports, with its buffers and parameters.
pub struct KernelAir {
    /// The program's identity ([`SdfSplashExpr::structure_key`]).
    pub key: u64,
    program: air::Program,
    points: u8,
    dist: u8,
    /// The compiler's control word buffer (the cancel token): reads 0.
    control: Option<u8>,
    pub params: usize,
    /// Worst-case AIR ops per sample.
    pub cost: u64,
}

impl KernelAir {
    /// The GPU route of a Splash field, or why it has none.
    pub fn new(splash: &SdfSplashExpr) -> Result<KernelAir, String> {
        let kernel = splash.kernel();
        let cost = kernel.cost;
        if cost > MAX_KERNEL_COST {
            return Err(format!("{cost} ops per sample (at most {MAX_KERNEL_COST} run on the GPU)"));
        }
        let params = kernel.param_count();
        if params > MAX_KERNEL_PARAMS {
            return Err(format!("{params} parameters (at most {MAX_KERNEL_PARAMS})"));
        }
        let (Some(points), Some(dist)) = (kernel.buffer_index("sdf_points"), kernel.buffer_index("sdf_dist")) else {
            return Err("not a field kernel (sdf_points in, sdf_dist out)".into());
        };
        let program = air::flat(kernel.program()).into_owned();
        if program.frame_words > MAX_FRAME_WORDS {
            return Err(format!("{} words of local arrays (at most {MAX_FRAME_WORDS})", program.frame_words));
        }
        let control = kernel.buffer_index(makepad_script_compute::lower::kernel::CONTROL_BUFFER).map(|c| c as u8);
        let k = KernelAir { key: splash.structure_key(), program, points: points as u8, dist: dist as u8, control, params, cost };
        // A dry run of the translation: what the shader would refuse, refused now.
        let mut ir = IrModule::new();
        k.function(&mut ir, "check".into())?;
        Ok(k)
    }

    /// The shader function `name(p: vec3, a: vec4, b: vec4, c: vec4, d: vec4) -> f32`:
    /// the kernel's distance at `p`, parameter k in lane k % 4 of a..d.
    fn function(&self, ir: &mut IrModule, name: String) -> Result<IrFunction, String> {
        let v3 = ir.vec_ty(3, IrScalar::F32);
        let v4 = ir.vec_ty(4, IrScalar::F32);
        let mut f = IrFunction::new(name, TY_F32);
        f.params.push(IrParam { name: id!(p), shadow: 0, ty: v3, inout: false });
        for n in [id!(a), id!(b), id!(c), id!(d)] {
            f.params.push(IrParam { name: n, shadow: 0, ty: v4, inout: false });
        }
        f.cost = self.cost;
        let frame_ty = (self.program.frame_words > 0).then(|| ir.ty(IrTy::Array(TY_I32, self.program.frame_words)));
        let mut t = Tr { k: self, f, vals: Vec::new(), vars: Vec::new(), frame: None, result: 0, jump: None };
        let mut body = Vec::new();
        for ty in self.program.vals.clone() {
            let (l, s) = t.declare(ty)?;
            t.vals.push(l);
            body.push(s);
        }
        for ty in self.program.vars.clone() {
            let (l, s) = t.declare(ty)?;
            t.vars.push(l);
            body.push(s);
        }
        if let Some(fty) = frame_ty {
            let zero = t.lit_i(0);
            let items = vec![zero; self.program.frame_words as usize];
            let init = t.f.expr(ExprKind::Construct(items), fty);
            let l = t.f.local(id!(frame), 0, fty, true, LocalKind::Temp);
            body.push(Stmt::Local(l, init));
            t.frame = Some(l);
        }
        let (r, s) = t.declare(Ty::F32)?;
        t.result = r;
        body.push(s);
        if has_deep_jump(&self.program.body) {
            let (j, s) = t.declare(Ty::I32)?;
            t.jump = Some(j);
            body.push(s);
        }
        let main = t.block(&self.program.body, 0)?;
        body.extend(main);
        let r = t.local(t.result, TY_F32);
        // + 0.0: a zero leaves as +0 (the volume's convention)
        let z = t.lit_f(0.0);
        let r = t.f.expr(ExprKind::Binary(BinOp::Add, r, z), TY_F32);
        body.push(Stmt::Return(r));
        t.f.body = body;
        Ok(t.f)
    }
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

/// One translation: AIR values and variables as shader locals.
struct Tr<'a> {
    k: &'a KernelAir,
    f: IrFunction,
    vals: Vec<LocalId>,
    vars: Vec<LocalId>,
    frame: Option<LocalId>,
    result: LocalId,
    /// Multi-level breaks: 2 d (+1 for a continue) of the loop to leave.
    jump: Option<LocalId>,
}

fn ir_ty(t: Ty) -> Result<TyId, String> {
    match t {
        Ty::F32 => Ok(TY_F32),
        Ty::I32 => Ok(TY_I32),
        Ty::Bool => Ok(TY_BOOL),
        Ty::F64 => Err("f64 arithmetic (no GPU route)".into()),
    }
}

impl Tr<'_> {
    fn e(&mut self, kind: ExprKind, ty: TyId) -> ExprId {
        self.f.expr(kind, ty)
    }
    fn local(&mut self, l: LocalId, ty: TyId) -> ExprId {
        self.e(ExprKind::Local(l), ty)
    }
    fn lit_f(&mut self, x: f32) -> ExprId {
        if x == 0.0 || x.is_normal() {
            self.e(ExprKind::Lit(IrLit::F32(x)), TY_F32)
        } else {
            // inf, nan and subnormals by their bits (the text forms are not exact)
            let b = self.lit_i(x.to_bits() as i32);
            self.e(ExprKind::Bitcast(b), TY_F32)
        }
    }
    fn lit_i(&mut self, x: i32) -> ExprId {
        self.e(ExprKind::Lit(IrLit::I32(x)), TY_I32)
    }
    fn lit_u(&mut self, x: u32) -> ExprId {
        self.e(ExprKind::Lit(IrLit::U32(x)), TY_U32)
    }
    fn lit_b(&mut self, x: bool) -> ExprId {
        self.e(ExprKind::Lit(IrLit::Bool(x)), TY_BOOL)
    }
    fn bin(&mut self, op: BinOp, a: ExprId, b: ExprId, ty: TyId) -> ExprId {
        self.e(ExprKind::Binary(op, a, b), ty)
    }
    fn sel(&mut self, c: ExprId, a: ExprId, b: ExprId, ty: TyId) -> ExprId {
        self.e(ExprKind::Select(c, a, b), ty)
    }
    fn call(&mut self, op: Builtin, args: Vec<ExprId>, ty: TyId) -> ExprId {
        self.e(ExprKind::Builtin(op, args), ty)
    }
    fn as_u(&mut self, a: ExprId) -> ExprId {
        self.e(ExprKind::Bitcast(a), TY_U32)
    }
    fn as_i(&mut self, a: ExprId) -> ExprId {
        self.e(ExprKind::Bitcast(a), TY_I32)
    }
    /// An i32 op done on u32 bits (wrapping, never signed overflow).
    fn wrapping(&mut self, op: BinOp, a: ExprId, b: ExprId) -> ExprId {
        let (ua, ub) = (self.as_u(a), self.as_u(b));
        let r = self.bin(op, ua, ub, TY_U32);
        self.as_i(r)
    }
    fn val(&mut self, v: air::Val) -> ExprId {
        let ty = ir_ty(self.k.program.vals[v.0 as usize]).unwrap_or(TY_F32);
        self.local(self.vals[v.0 as usize], ty)
    }
    fn declare(&mut self, ty: Ty) -> Result<(LocalId, Stmt), String> {
        let t = ir_ty(ty)?;
        let zero = match ty {
            Ty::F32 => self.lit_f(0.0),
            Ty::I32 => self.lit_i(0),
            _ => self.lit_b(false),
        };
        let l = self.f.local(id!(k), 0, t, true, LocalKind::Temp);
        Ok((l, Stmt::Local(l, zero)))
    }
    fn param(&mut self, k: usize) -> ExprId {
        let v4 = self.f.params[1 + k / 4].ty;
        let v = self.e(ExprKind::Param(1 + (k / 4) as u32), v4);
        self.e(ExprKind::Swizzle(v, [(k % 4) as u8, 0, 0, 0], 1), TY_F32)
    }
    fn point(&mut self, c: u8) -> ExprId {
        let v3 = self.f.params[0].ty;
        let p = self.e(ExprKind::Param(0), v3);
        self.e(ExprKind::Swizzle(p, [c, 0, 0, 0], 1), TY_F32)
    }
    /// A word as a value of type `ty` (`w` is an i32 or f32 expression of the word's bits).
    fn word_as(&mut self, w: ExprId, w_is_f: bool, ty: TyId) -> ExprId {
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
    fn to_word(&mut self, v: air::Val) -> ExprId {
        let ty = self.k.program.vals[v.0 as usize];
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

    fn op(&mut self, op: &Op, ty: TyId) -> Result<ExprId, String> {
        let k = self.k;
        Ok(match *op {
            Op::ConstF(x) => self.lit_f(x),
            Op::ConstI(x) => self.lit_i(x),
            Op::ConstB(x) => self.lit_b(x),
            Op::ConstD(_) => return Err("f64 constant".into()),
            Op::Get(v) => {
                let t = ir_ty(k.program.vars[v.0 as usize])?;
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
                let p0 = makepad_script_compute::lower::kernel::K_PARAMS;
                if base >= p0 && ((base - p0) as usize) < k.params {
                    let p = self.param((base - p0) as usize);
                    self.word_as(p, true, ty)
                } else if base == makepad_script_compute::lower::kernel::K_COUNT {
                    let one = self.lit_i(1);
                    self.word_as(one, false, ty)
                } else {
                    let z = self.lit_i(0);
                    self.word_as(z, false, ty)
                }
            }
            Op::Load { region: Region::Buf(b), base, off, .. } if b == k.points => {
                // the one element's point: words 0..2, clamped as buf_index does
                let w = match off {
                    None => {
                        let c = base.min(2) as u8;
                        self.point(c)
                    }
                    Some(o) => {
                        let o = self.val(o);
                        let ou = self.as_u(o);
                        let b = self.lit_u(base);
                        let i = self.bin(BinOp::Add, ou, b, TY_U32);
                        let (z, one) = (self.lit_u(0), self.lit_u(1));
                        let is0 = self.bin(BinOp::Eq, i, z, TY_BOOL);
                        let is1 = self.bin(BinOp::Eq, i, one, TY_BOOL);
                        let (px, py, pz) = (self.point(0), self.point(1), self.point(2));
                        let yz = self.sel(is1, py, pz, TY_F32);
                        self.sel(is0, px, yz, TY_F32)
                    }
                };
                self.word_as(w, true, ty)
            }
            Op::Load { region: Region::Buf(b), .. } if Some(b) == k.control => {
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
            Op::Load { region, .. } => return Err(format!("a read of {region:?} memory")),
            Op::In { .. } => return Err("an audio input".into()),
            Op::FrameCount => self.lit_i(1),
            Op::BufLen(b) => self.lit_i(if b == k.points { 3 } else { 1 }),
        })
    }

    fn block(&mut self, b: &air::Block, depth: u32) -> Result<Vec<Stmt>, String> {
        let k = self.k;
        let mut out = Vec::new();
        for s in b {
            match s {
                air::Stmt::Def(v, op) => {
                    let ty = ir_ty(k.program.vals[v.0 as usize])?;
                    let e = self.op(op, ty)?;
                    let l = self.local(self.vals[v.0 as usize], ty);
                    out.push(Stmt::Assign(l, e));
                }
                air::Stmt::Set(var, v) => {
                    let ty = ir_ty(k.program.vars[var.0 as usize])?;
                    let e = self.val(*v);
                    let l = self.local(self.vars[var.0 as usize], ty);
                    out.push(Stmt::Assign(l, e));
                }
                air::Stmt::Store { region: Region::Buf(b), val, .. } if *b == k.dist => {
                    let e = self.val(*val);
                    let e = match k.program.vals[val.0 as usize] {
                        Ty::F32 => e,
                        Ty::I32 => self.e(ExprKind::Bitcast(e), TY_F32),
                        _ => return Err("a bool distance".into()),
                    };
                    let l = self.local(self.result, TY_F32);
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
                air::Stmt::Store { region, .. } => return Err(format!("a write to {region:?} memory")),
                air::Stmt::Out { .. } => return Err("an audio output".into()),
                air::Stmt::If(c, t, e) => {
                    let c = self.val(*c);
                    let t = self.block(t, depth)?;
                    let e = self.block(e, depth)?;
                    out.push(Stmt::If(c, t, e));
                }
                air::Stmt::Loop { cap, body } => {
                    let g = self.f.local(id!(g), 0, TY_U32, true, LocalKind::Guard);
                    let body = self.block(body, depth + 1)?;
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

/// One kernel of a volume (by program identity).
#[derive(Clone)]
pub struct KernelSlot {
    /// The program's identity (or the field's key when it is no Splash program).
    pub key: u64,
    field: Arc<dyn Sdf3 + Send + Sync>,
    splash: Option<Arc<SdfSplashExpr>>,
    /// The GPU route; `None`: its bricks bake on the CPU twin.
    pub gpu: Option<Arc<KernelAir>>,
}

fn structure_key(k: &SdfKernel) -> u64 {
    k.splash.as_ref().map_or(k.key, |s| s.structure_key())
}

impl KernelSlot {
    pub fn new(k: &SdfKernel) -> KernelSlot {
        let gpu = k.splash.as_ref().and_then(|s| match KernelAir::new(s) {
            Ok(a) => Some(Arc::new(a)),
            Err(e) => {
                log!("sdf engine: kernel {:016x} bakes on the CPU: {e}", structure_key(k));
                None
            }
        });
        KernelSlot { key: structure_key(k), field: k.field.clone(), splash: k.splash.clone(), gpu }
    }

    /// Whether `k` runs this slot's program.
    pub fn matches(&self, k: &SdfKernel) -> bool {
        self.key == structure_key(k)
    }

    /// [`KernelSlot::distance`] at many points (one kernel call).
    pub fn distance_many(&self, params: &[f32], ps: &[[f32; 3]]) -> Vec<f32> {
        match &self.splash {
            Some(s) => {
                let xyz: Vec<f32> = ps.iter().flatten().copied().collect();
                let mut out = vec![0.0f32; ps.len()];
                s.distance_batch_with(params, &xyz, &mut out);
                out
            }
            None => ps.iter().map(|p| self.distance(params, *p)).collect(),
        }
    }

    /// The distance at `p` with parameters `params` (by parameter index).
    pub fn distance(&self, params: &[f32], p: [f32; 3]) -> f32 {
        match &self.splash {
            Some(s) => {
                let mut out = [0.0f32];
                s.distance_batch_with(params, &p, &mut out);
                out[0]
            }
            None => self.field.distance(makepad_csg_math::Vec3d::new(p[0] as f64, p[1] as f64, p[2] as f64)) as f32,
        }
    }
}

/// Whether `n` calls any kernel.
pub fn has_kernel(n: &SdfNode) -> bool {
    match n {
        SdfNode::Kernel(_) => true,
        SdfNode::Combine { of, .. } => of.iter().any(has_kernel),
        SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Instance { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Deform { of, .. } | SdfNode::Id { of, .. } => has_kernel(of),
        SdfNode::Material { within, of, .. } => within.as_deref().is_some_and(has_kernel) || has_kernel(of),
        _ => false,
    }
}

/// Adds the kernels of `n` missing from `slots` (their order is stable:
/// a slot keeps its index while its volume lives).
pub fn collect(n: &SdfNode, slots: &mut Vec<KernelSlot>) {
    match n {
        SdfNode::Kernel(k) => {
            if !slots.iter().any(|s| s.matches(k)) {
                slots.push(KernelSlot::new(k));
            }
        }
        SdfNode::Combine { of, .. } => of.iter().for_each(|c| collect(c, slots)),
        SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Instance { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Deform { of, .. } | SdfNode::Id { of, .. } => collect(of, slots),
        SdfNode::Material { within, of, .. } => {
            if let Some(w) = within {
                collect(w, slots);
            }
            collect(of, slots);
        }
        _ => {}
    }
}

/// The bake shader's kernel dispatch: `kernel_call(slot, p, a, b, c, d)`
/// over a volume's GPU kernels.
struct Dispatch {
    key: u64,
    kernels: Vec<Option<Arc<KernelAir>>>,
}

impl ShaderIrFn for Dispatch {
    fn key(&self) -> u64 {
        self.key
    }

    fn build(&self, ir: &mut IrModule, f: &mut IrFunction) -> Result<(), String> {
        if f.params.len() != 6 {
            return Err(format!("kernel_call takes (slot, p, a, b, c, d), not {} params", f.params.len()));
        }
        let mut body = Vec::new();
        for (i, k) in self.kernels.iter().enumerate() {
            let Some(k) = k else { continue };
            let kf = k.function(ir, format!("sdfk_{:016x}", k.key))?;
            ir.functions.push(kf);
            let id = (ir.functions.len() - 1) as FuncId;
            let slot = f.expr(ExprKind::Param(0), TY_F32);
            let lim = f.expr(ExprKind::Lit(IrLit::F32(i as f32 + 0.5)), TY_F32);
            let here = f.expr(ExprKind::Binary(BinOp::Lt, slot, lim), TY_BOOL);
            let args: Vec<ExprId> = (1..6).map(|p| {
                let ty = f.params[p].ty;
                f.expr(ExprKind::Param(p as u32), ty)
            }).collect();
            let call = f.expr(ExprKind::Call(id, args), TY_F32);
            body.push(Stmt::If(here, vec![Stmt::Return(call)], Vec::new()));
            f.cost = f.cost.max(k.cost);
        }
        let far = f.expr(ExprKind::Lit(IrLit::F32(1.0e9)), TY_F32);
        body.push(Stmt::Return(far));
        f.body = body;
        Ok(())
    }
}

/// A volume's shaders with its kernels compiled in: the bake's, and the
/// march's (made from the march its host draws with, once asked for).
pub struct KernelBake {
    /// The kernels' program identities (slot order) it was made for.
    pub keys: Vec<u64>,
    pub draw: DrawSdfBake,
    /// The march with the kernels (exact evaluation near the surface), and
    /// the shader it was made from.
    march: Option<(DrawShaderId, DrawSdfMarch)>,
    /// Every GPU kernel is cheap enough to evaluate in the march.
    pub march_ok: bool,
    stub: ScriptObject,
    refs: Vec<ScriptObjectRef>,
}

/// Worst-case ops per sample of a kernel the march evaluates exactly (a ray
/// may evaluate it at every step in the band and four times for its
/// normal); costlier kernels march their baked copy.
pub const MAX_MARCH_KERNEL_COST: u64 = 2_000;

/// `base` with the kernels' dispatch `own` and the fetching kernel_at.
fn variant(vm: &mut ScriptVm, base: ScriptObject, own: ScriptObject) -> Option<ScriptObject> {
    let fetching = vm.value(base, id!(kernel_at_fetch)).as_object()?;
    let obj = vm.new_with_proto(base);
    vm.set_value(obj, id!(kernel_call), own);
    vm.set_value(obj, id!(kernel_at), fetching);
    Some(obj)
}

impl KernelBake {
    /// The bake variant for `slots` (`None` when none has a GPU route).
    pub fn new(cx: &mut Cx, slots: &[KernelSlot]) -> Option<KernelBake> {
        if slots.iter().all(|s| s.gpu.is_none()) {
            return None;
        }
        let keys: Vec<u64> = slots.iter().map(|s| s.key).collect();
        let mut key = 0xcbf2_9ce4_8422_2325u64;
        for (k, s) in keys.iter().zip(slots) {
            key = (key ^ k ^ s.gpu.is_some() as u64).wrapping_mul(0x100_0000_01b3);
        }
        let march_ok = slots.iter().all(|s| s.gpu.as_ref().is_some_and(|g| g.cost <= MAX_MARCH_KERNEL_COST));
        let dispatch = Rc::new(Dispatch { key, kernels: slots.iter().map(|s| s.gpu.clone()).collect() });
        cx.with_vm(|vm| {
            let draw_mod = vm.heap().module(id!(draw));
            let base = vm.value(draw_mod, id!(SdfBake)).as_object()?;
            let stub = vm.value(base, id!(kernel_call)).as_object()?;
            // a fn object of its own: the shader's identity and the hook's key
            let proto = vm.heap().proto(stub);
            let own = vm.new_with_proto_of(proto);
            vm.heap_mut().vec_push_vec(own, stub, NoTrap);
            let ptr = vm.heap().as_fn(stub)?;
            vm.heap_mut().set_fn(own, ptr);
            vm.set_shader_ir_fn(own, Some(dispatch.clone() as Rc<dyn ShaderIrFn>));
            let obj = variant(vm, base, own)?;
            let refs = vec![vm.new_object_ref(own), vm.new_object_ref(obj)];
            let draw = DrawSdfBake::script_from_value(vm, obj.into());
            Some(KernelBake { keys, draw, march: None, march_ok, stub: own, refs })
        })
    }

    /// The march `base` with the kernels compiled in (made once per base
    /// shader); `None` when a kernel is too costly to evaluate per step.
    pub fn march(&mut self, cx: &mut Cx, base: &DrawSdfMarch) -> Option<&mut DrawSdfMarch> {
        if !self.march_ok {
            return None;
        }
        let id = base.draw_super.draw_vars.draw_shader_id?;
        if self.march.as_ref().is_none_or(|(b, _)| *b != id) {
            let source = cx.draw_shaders.shaders.get(id.index)?.mapping.source.as_object();
            let own = self.stub;
            let made = cx.with_vm(|vm| {
                let obj = variant(vm, source, own)?;
                let r = vm.new_object_ref(obj);
                let mut draw = DrawSdfMarch::script_from_value(vm, obj.into());
                draw.kernels = true;
                Some((r, draw))
            });
            let (r, draw) = made?;
            self.refs.push(r);
            self.march = Some((id, draw));
        }
        self.march.as_mut().map(|(_, d)| d)
    }

    /// Whether this variant runs `slots`' kernels.
    pub fn serves(&self, slots: &[KernelSlot]) -> bool {
        self.keys.len() == slots.len() && self.keys.iter().zip(slots).all(|(k, s)| *k == s.key)
    }

    pub fn release(self, cx: &mut Cx) {
        let stub = self.stub;
        cx.with_vm(|vm| vm.set_shader_ir_fn(stub, None));
    }
}

