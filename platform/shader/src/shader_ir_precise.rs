//! Correctly rounded division and square root for `precise_math` shaders.
//!
//! GPU division and sqrt are approximations on most backends (Vulkan allows
//! 2.5 ulp for `x / y` and 3 ulp for `sqrt`; measured on an RTX 5090: 1 ulp
//! off on about one value in five), while a CPU reference (`a / b`,
//! `f32::sqrt`) rounds correctly. A precise module therefore lowers every f32
//! division and square root (and the builtins built on them: `length`,
//! `distance`, `normalize`, `inverseSqrt`, and `dot`, whose summation order is
//! fixed) to small IR functions that take the hardware's result and correct it
//! to the nearest float with exact arithmetic:
//!
//! - the product `q * b` is split exactly into `p + e` (Dekker's product: a
//!   Veltkamp split by 4097 and four partial products, every op one rounding;
//!   the module is precise, so no backend fuses or reassociates them);
//! - the remainder `r = a - q b = (a - p) - e` is then exact for a candidate
//!   within a few ulp of the quotient (`a - p` by Sterbenz, and the remainder
//!   of a faithful quotient is a float);
//! - `q` is the nearest float iff `2 |r| < |b| * gap`, the gap to its
//!   neighbour on the remainder's side (a power of two: the product is exact;
//!   a quotient is never a midpoint), else the neighbour is tried, at most
//!   three times. Square root alike: `r = x - s s`, `s` is nearest iff
//!   `r <= s * gap_up` (r >= 0) or `-r < s * gap_down` (r < 0), exact
//!   because both sides lie on the grid of `ulp(s)^2`.
//!
//! Operands outside [2^-60, 2^60] (and zero, inf, NaN) keep the hardware
//! result: the split would overflow there or the partial products underflow.
//! Distances, lattice coordinates and blend radii of a bake live far inside.
//!
//! The fixed forms a CPU twin writes the same way:
//! `dot(a, b) = ((a.x * b.x + a.y * b.y) + a.z * b.z) + a.w * b.w`,
//! `length(v) = sqrt(dot(v, v))`, `distance(a, b) = length(a - b)`,
//! `normalize(v) = v / length(v)` (per component), `inverseSqrt(x) = 1 / sqrt(x)`.
//! Other transcendental builtins (`pow`, `exp`, `mix`, `smoothstep`, ...) keep
//! their backend precision: a bit-exact kernel writes them out.

use crate::shader_ir::*;
use makepad_live_id::{id, LiveId};

/// How a precise module divides and takes roots: through the correcting helpers (their
/// function ids), or with the backend's own ops where those already round correctly.
struct Helpers {
    correct: Option<(FuncId, FuncId)>,
}

impl Helpers {
    fn div(&self, f: &mut IrFunction, a: ExprId, b: ExprId) -> ExprId {
        match self.correct {
            Some((div, _)) => f.expr(ExprKind::Call(div, vec![a, b]), TY_F32),
            None => f.expr(ExprKind::Binary(BinOp::Div, a, b), TY_F32),
        }
    }
    fn sqrt(&self, f: &mut IrFunction, x: ExprId) -> ExprId {
        match self.correct {
            Some((_, sqrt)) => f.expr(ExprKind::Call(sqrt, vec![x]), TY_F32),
            None => f.expr(ExprKind::Builtin(Builtin::Sqrt, vec![x]), TY_F32),
        }
    }
}

const LO: f32 = 8.673617e-19; // 2^-60
const HI: f32 = 1.1529215e18; // 2^60

struct B {
    f: IrFunction,
}

impl B {
    fn e(&mut self, kind: ExprKind, ty: TyId) -> ExprId {
        self.f.expr(kind, ty)
    }
    fn f(&mut self, v: f32) -> ExprId {
        self.e(ExprKind::Lit(IrLit::F32(v)), TY_F32)
    }
    fn u(&mut self, v: u32) -> ExprId {
        self.e(ExprKind::Lit(IrLit::U32(v)), TY_U32)
    }
    fn param(&mut self, i: u32) -> ExprId {
        self.e(ExprKind::Param(i), TY_F32)
    }
    fn bin(&mut self, op: BinOp, a: ExprId, b: ExprId) -> ExprId {
        let ty = if op.is_compare() { TY_BOOL } else { self.f.ty_of(a) };
        self.e(ExprKind::Binary(op, a, b), ty)
    }
    fn and(&mut self, a: ExprId, b: ExprId) -> ExprId {
        self.e(ExprKind::Binary(BinOp::LogicAnd, a, b), TY_BOOL)
    }
    fn not(&mut self, a: ExprId) -> ExprId {
        self.e(ExprKind::Unary(UnOp::Not, a), TY_BOOL)
    }
    fn abs(&mut self, a: ExprId) -> ExprId {
        self.e(ExprKind::Builtin(Builtin::Abs, vec![a]), TY_F32)
    }
    /// A new f32 variable holding `init`; returns (local, read expr).
    fn var(&mut self, name: LiveId, init: ExprId, body: &mut Vec<Stmt>) -> (ExprId, LocalId) {
        let l = self.f.local(name, self.f.locals.len() as u32, TY_F32, true, LocalKind::Temp);
        body.push(Stmt::Local(l, init));
        (self.e(ExprKind::Local(l), TY_F32), l)
    }
    /// `let` of an f32 value (evaluated once).
    fn val(&mut self, init: ExprId, body: &mut Vec<Stmt>) -> ExprId {
        let l = self.f.local(id!(_mp_v), self.f.locals.len() as u32, TY_F32, false, LocalKind::Temp);
        body.push(Stmt::Local(l, init));
        self.e(ExprKind::Local(l), TY_F32)
    }
    /// The neighbouring float of positive finite `v`: one bit pattern up or down.
    fn step(&mut self, v: ExprId, up: bool) -> ExprId {
        let bits = self.e(ExprKind::Bitcast(v), TY_U32);
        let one = self.u(1);
        let moved = self.e(ExprKind::Binary(if up { BinOp::Add } else { BinOp::Sub }, bits, one), TY_U32);
        self.e(ExprKind::Bitcast(moved), TY_F32)
    }
    /// `(lo < |v|) && (|v| < hi)` (false for zero, inf, NaN).
    fn in_range(&mut self, v: ExprId) -> ExprId {
        let a = self.abs(v);
        let lo = self.f(LO);
        let hi = self.f(HI);
        let l = self.bin(BinOp::Gt, a, lo);
        let h = self.bin(BinOp::Lt, a, hi);
        self.and(l, h)
    }
    /// `a - q * b` exactly (for `q * b` within a factor two of `a`), into `body`.
    fn remainder(&mut self, a: ExprId, q: ExprId, b: ExprId, body: &mut Vec<Stmt>) -> ExprId {
        let p = self.bin(BinOp::Mul, q, b);
        let p = self.val(p, body);
        let (qh, ql) = self.split(q, body);
        let (bh, bl) = self.split(b, body);
        let hh = self.bin(BinOp::Mul, qh, bh);
        let t = self.bin(BinOp::Sub, hh, p);
        let hl = self.bin(BinOp::Mul, qh, bl);
        let t = self.bin(BinOp::Add, t, hl);
        let lh = self.bin(BinOp::Mul, ql, bh);
        let t = self.bin(BinOp::Add, t, lh);
        let ll = self.bin(BinOp::Mul, ql, bl);
        let err = self.bin(BinOp::Add, t, ll);
        let err = self.val(err, body);
        let d = self.bin(BinOp::Sub, a, p);
        let r = self.bin(BinOp::Sub, d, err);
        self.val(r, body)
    }
    /// Veltkamp's split of `v` into a 12-bit high part and the rest.
    fn split(&mut self, v: ExprId, body: &mut Vec<Stmt>) -> (ExprId, ExprId) {
        let k = self.f(4097.0);
        let c = self.bin(BinOp::Mul, k, v);
        let c = self.val(c, body);
        let cv = self.bin(BinOp::Sub, c, v);
        let hi = self.bin(BinOp::Sub, c, cv);
        let hi = self.val(hi, body);
        let lo = self.bin(BinOp::Sub, v, hi);
        let lo = self.val(lo, body);
        (hi, lo)
    }
}

/// `_mp_precise_div(a, b)`: the nearest float to a / b.
fn build_div() -> IrFunction {
    let mut b = B { f: IrFunction::new("_mp_precise_div".to_string(), TY_F32) };
    b.f.params = vec![
        IrParam { name: id!(a), shadow: 0, ty: TY_F32, inout: false },
        IrParam { name: id!(b), shadow: 0, ty: TY_F32, inout: false },
    ];
    let mut body = Vec::new();
    let a = b.param(0);
    let bb = b.param(1);
    let q = b.bin(BinOp::Div, a, bb);
    let q = b.val(q, &mut body);
    // outside the safe range: the hardware's quotient
    let ra = b.in_range(a);
    let rb = b.in_range(bb);
    let rq = b.in_range(q);
    let ok = b.and(ra, rb);
    let ok = b.and(ok, rq);
    let out = b.not(ok);
    body.push(Stmt::If(out, vec![Stmt::Return(q)], Vec::new()));
    let aa = b.abs(a);
    let aa = b.val(aa, &mut body);
    let ab = b.abs(bb);
    let ab = b.val(ab, &mut body);
    let aq = b.abs(q);
    let (qv, ql) = b.var(id!(_mp_q), aq, &mut body);
    let counter = b.f.local(id!(_mp_i), b.f.locals.len() as u32, TY_U32, true, LocalKind::Guard);
    let mut lp = Vec::new();
    let r = b.remainder(aa, qv, ab, &mut lp);
    let zero = b.f(0.0);
    let nonneg = b.bin(BinOp::Ge, r, zero);
    let two = b.f(2.0);
    // r >= 0: nearest iff 2r < |b| (up(q) - q), else up
    let mut pos = Vec::new();
    let up = b.step(qv, true);
    let up = b.val(up, &mut pos);
    let gap = b.bin(BinOp::Sub, up, qv);
    let lim = b.bin(BinOp::Mul, ab, gap);
    let r2 = b.bin(BinOp::Mul, two, r);
    let near = b.bin(BinOp::Lt, r2, lim);
    let place = b.e(ExprKind::Local(ql), TY_F32);
    pos.push(Stmt::If(near, vec![Stmt::Break], vec![Stmt::Assign(place, up)]));
    // r < 0: nearest iff -2r < |b| (q - down(q)), else down
    let mut neg = Vec::new();
    let down = b.step(qv, false);
    let down = b.val(down, &mut neg);
    let gap = b.bin(BinOp::Sub, qv, down);
    let lim = b.bin(BinOp::Mul, ab, gap);
    let r2 = b.bin(BinOp::Mul, two, r);
    let nr2 = b.e(ExprKind::Unary(UnOp::Neg, r2), TY_F32);
    let near = b.bin(BinOp::Lt, nr2, lim);
    let place = b.e(ExprKind::Local(ql), TY_F32);
    neg.push(Stmt::If(near, vec![Stmt::Break], vec![Stmt::Assign(place, down)]));
    lp.push(Stmt::If(nonneg, pos, neg));
    let start = b.u(0);
    let end = b.u(3);
    body.push(Stmt::For(counter, start, end, lp));
    // the sign of the quotient (an exact multiply by -1 / 1)
    let neg_q = b.bin(BinOp::Lt, q, zero);
    let nq = b.e(ExprKind::Unary(UnOp::Neg, qv), TY_F32);
    let res = b.e(ExprKind::Select(neg_q, nq, qv), TY_F32);
    body.push(Stmt::Return(res));
    b.f.body = body;
    b.f
}

/// `_mp_precise_sqrt(x)`: the nearest float to sqrt(x).
fn build_sqrt() -> IrFunction {
    let mut b = B { f: IrFunction::new("_mp_precise_sqrt".to_string(), TY_F32) };
    b.f.params = vec![IrParam { name: id!(x), shadow: 0, ty: TY_F32, inout: false }];
    let mut body = Vec::new();
    let x = b.param(0);
    let s = b.e(ExprKind::Builtin(Builtin::Sqrt, vec![x]), TY_F32);
    let s = b.val(s, &mut body);
    // zero, negative, inf, NaN and the ends of the range: the hardware's root
    let rx = b.in_range(x);
    let zero = b.f(0.0);
    let posx = b.bin(BinOp::Gt, x, zero);
    let ok = b.and(rx, posx);
    let out = b.not(ok);
    body.push(Stmt::If(out, vec![Stmt::Return(s)], Vec::new()));
    let (sv, sl) = b.var(id!(_mp_s), s, &mut body);
    let counter = b.f.local(id!(_mp_i), b.f.locals.len() as u32, TY_U32, true, LocalKind::Guard);
    let mut lp = Vec::new();
    let r = b.remainder(x, sv, sv, &mut lp);
    let nonneg = b.bin(BinOp::Ge, r, zero);
    // r >= 0: nearest iff r <= s (up(s) - s), else up
    let mut pos = Vec::new();
    let up = b.step(sv, true);
    let up = b.val(up, &mut pos);
    let gap = b.bin(BinOp::Sub, up, sv);
    let lim = b.bin(BinOp::Mul, sv, gap);
    let near = b.bin(BinOp::Le, r, lim);
    let place = b.e(ExprKind::Local(sl), TY_F32);
    pos.push(Stmt::If(near, vec![Stmt::Break], vec![Stmt::Assign(place, up)]));
    // r < 0: nearest iff -r < s (s - down(s)), else down
    let mut neg = Vec::new();
    let down = b.step(sv, false);
    let down = b.val(down, &mut neg);
    let gap = b.bin(BinOp::Sub, sv, down);
    let lim = b.bin(BinOp::Mul, sv, gap);
    let nr = b.e(ExprKind::Unary(UnOp::Neg, r), TY_F32);
    let near = b.bin(BinOp::Lt, nr, lim);
    let place = b.e(ExprKind::Local(sl), TY_F32);
    neg.push(Stmt::If(near, vec![Stmt::Break], vec![Stmt::Assign(place, down)]));
    lp.push(Stmt::If(nonneg, pos, neg));
    let start = b.u(0);
    let end = b.u(3);
    body.push(Stmt::For(counter, start, end, lp));
    body.push(Stmt::Return(sv));
    b.f.body = body;
    b.f
}

/// Lowers a precise module for one backend (see the module doc): the fixed forms of dot,
/// length, distance and normalize everywhere, and, where `correct_div_sqrt` (every backend
/// but Metal, whose IEEE mode without fast math already rounds both correctly: measured on
/// Apple M-series, 0 of 65536 differing over 31 binades), every f32 division and square root
/// through the correcting helpers. Each backend lowers its own copy of the module once.
pub fn lower_precise(m: &mut IrModule, correct_div_sqrt: bool) {
    if !m.precise {
        return;
    }
    let count = m.functions.len();
    let h = if correct_div_sqrt {
        m.functions.push(build_div());
        m.functions.push(build_sqrt());
        Helpers { correct: Some((count as FuncId, count as FuncId + 1)) }
    } else {
        Helpers { correct: None }
    };
    for fi in 0..count {
        let mut f = std::mem::replace(&mut m.functions[fi], IrFunction::new(String::new(), TY_VOID));
        lower_function(m, &mut f, &h);
        m.functions[fi] = f;
    }
}

fn is_f32(m: &IrModule, ty: TyId) -> bool {
    matches!(m.get(ty), IrTy::Scalar(IrScalar::F32) | IrTy::Vec(_, IrScalar::F32))
}

fn lanes(m: &IrModule, ty: TyId) -> u8 {
    match m.get(ty) {
        IrTy::Vec(n, _) => *n,
        _ => 1,
    }
}

/// Lane `i` of `e` (the value itself when it is a scalar).
fn lane(m: &IrModule, f: &mut IrFunction, e: ExprId, i: u8) -> ExprId {
    if lanes(m, f.ty_of(e)) == 1 {
        return e;
    }
    f.expr(ExprKind::Swizzle(e, [i, 0, 0, 0], 1), TY_F32)
}

/// `op` over the lanes of `ty`, from per-lane scalars.
fn per_lane(m: &IrModule, f: &mut IrFunction, ty: TyId, mut op: impl FnMut(&mut IrFunction, u8) -> ExprId) -> ExprKind {
    let n = lanes(m, ty);
    if n == 1 {
        let e = op(f, 0);
        return f.exprs[e as usize].kind.clone();
    }
    let parts = (0..n).map(|i| op(f, i)).collect();
    ExprKind::Construct(parts)
}

/// `((a.x * b.x + a.y * b.y) + a.z * b.z) + a.w * b.w` of two f32 vectors.
fn dot_chain(m: &IrModule, f: &mut IrFunction, a: ExprId, b: ExprId) -> ExprId {
    let n = lanes(m, f.ty_of(a)).max(lanes(m, f.ty_of(b)));
    let mut sum = NO_EXPR;
    for i in 0..n {
        let (x, y) = (lane(m, f, a, i), lane(m, f, b, i));
        let p = f.expr(ExprKind::Binary(BinOp::Mul, x, y), TY_F32);
        sum = if sum == NO_EXPR { p } else { f.expr(ExprKind::Binary(BinOp::Add, sum, p), TY_F32) };
    }
    sum
}

fn lower_function(m: &IrModule, f: &mut IrFunction, h: &Helpers) {
    // a compound `x /= y` becomes `x = x / y` first, then the expression pass lowers the division
    let mut body = std::mem::take(&mut f.body);
    lower_compound(m, f, &mut body);
    f.body = body;
    let original = f.exprs.len();
    for e in 0..original {
        let ty = f.exprs[e].ty;
        if !is_f32(m, ty) {
            continue;
        }
        let new_kind = match f.exprs[e].kind.clone() {
            ExprKind::Binary(BinOp::Div, a, b) if h.correct.is_some() => {
                Some(per_lane(m, f, ty, |f, i| {
                    let (x, y) = (lane(m, f, a, i), lane(m, f, b, i));
                    h.div(f, x, y)
                }))
            }
            ExprKind::Builtin(Builtin::Sqrt, args) if args.len() == 1 && h.correct.is_some() => {
                let a = args[0];
                Some(per_lane(m, f, ty, |f, i| {
                    let x = lane(m, f, a, i);
                    h.sqrt(f, x)
                }))
            }
            ExprKind::Builtin(Builtin::InverseSqrt, args) if args.len() == 1 => {
                let a = args[0];
                Some(per_lane(m, f, ty, |f, i| {
                    let x = lane(m, f, a, i);
                    let s = h.sqrt(f, x);
                    let one = f.expr(ExprKind::Lit(IrLit::F32(1.0)), TY_F32);
                    h.div(f, one, s)
                }))
            }
            ExprKind::Builtin(Builtin::Dot, args) if args.len() == 2 => {
                let d = dot_chain(m, f, args[0], args[1]);
                Some(f.exprs[d as usize].kind.clone())
            }
            ExprKind::Builtin(Builtin::Length, args) if args.len() == 1 => {
                let d = dot_chain(m, f, args[0], args[0]);
                let r = h.sqrt(f, d);
                Some(f.exprs[r as usize].kind.clone())
            }
            ExprKind::Builtin(Builtin::Distance, args) if args.len() == 2 => {
                let vt = f.ty_of(args[0]);
                let diff = f.expr(ExprKind::Binary(BinOp::Sub, args[0], args[1]), vt);
                let d = dot_chain(m, f, diff, diff);
                let r = h.sqrt(f, d);
                Some(f.exprs[r as usize].kind.clone())
            }
            ExprKind::Builtin(Builtin::Normalize, args) if args.len() == 1 => {
                let v = args[0];
                let d = dot_chain(m, f, v, v);
                let len = h.sqrt(f, d);
                Some(per_lane(m, f, ty, |f, i| {
                    let x = lane(m, f, v, i);
                    h.div(f, x, len)
                }))
            }
            _ => None,
        };
        if let Some(kind) = new_kind {
            f.exprs[e].kind = kind;
        }
    }
}

fn lower_compound(m: &IrModule, f: &mut IrFunction, stmts: &mut [Stmt]) {
    for s in stmts.iter_mut() {
        match s {
            Stmt::CompoundAssign(place, BinOp::Div, value) if is_f32(m, f.ty_of(*place)) => {
                let ty = f.ty_of(*place);
                let div = f.expr(ExprKind::Binary(BinOp::Div, *place, *value), ty);
                *s = Stmt::Assign(*place, div);
            }
            Stmt::If(_, a, b) => {
                lower_compound(m, f, a);
                lower_compound(m, f, b);
            }
            Stmt::For(_, _, _, body) | Stmt::Loop(body) => lower_compound(m, f, body),
            _ => {}
        }
    }
}
