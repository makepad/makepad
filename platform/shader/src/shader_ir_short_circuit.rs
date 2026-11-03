//! Statement lowering for text targets without scalar short-circuit operators.
use crate::shader_ir::*;
use makepad_live_id::LiveId;

fn contains(f: &IrFunction, e: ExprId) -> bool {
    if e == NO_EXPR { return false; }
    if f.needs_short_circuit(e) { return true; }
    let mut children = Vec::new();
    expr_children(&f.exprs[e as usize].kind, &mut children);
    children.into_iter().any(|e| contains(f, e))
}

pub(crate) fn needed(f: &IrFunction, stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Local(_, e) | Stmt::Expr(e) | Stmt::Return(e) => contains(f, *e),
        Stmt::Assign(a, b) | Stmt::CompoundAssign(a, _, b) => contains(f, *a) || contains(f, *b),
        Stmt::If(c, a, b) => contains(f, *c) || needed(f, a) || needed(f, b),
        Stmt::For(_, a, b, body) => contains(f, *a) || contains(f, *b) || needed(f, body),
        Stmt::Loop(body) => needed(f, body),
        _ => false,
    })
}

fn temp(f: &mut IrFunction, e: ExprId, out: &mut Vec<Stmt>) -> ExprId {
    let ty = f.ty_of(e);
    let l = f.local(LiveId(0), 0, ty, true, LocalKind::Temp);
    out.push(Stmt::Local(l, e));
    f.expr(ExprKind::Local(l), ty)
}

/// Keep addresses as addresses, freezing their indices before later effects.
fn place(f: &mut IrFunction, e: ExprId, out: &mut Vec<Stmt>) -> ExprId {
    let ex = f.exprs[e as usize].clone();
    let kind = match ex.kind {
        ExprKind::Field(a, n) => ExprKind::Field(place(f, a, out), n),
        ExprKind::Swizzle(a, lanes, n) => ExprKind::Swizzle(place(f, a, out), lanes, n),
        ExprKind::Index(a, i) => {
            let a = place(f, a, out);
            ExprKind::Index(a, value(f, i, out))
        }
        ExprKind::Local(_) | ExprKind::Global(_) | ExprKind::Param(_) | ExprKind::Deref(_) => return e,
        _ => return value(f, e, out),
    };
    f.expr(kind, ex.ty)
}

/// Normalize only a statement containing a branch-valued expression. Snapshot
/// its operands in order so a later branch cannot move ahead of an earlier call
/// or read. The backend compiler can eliminate the resulting scalar copies.
fn value(f: &mut IrFunction, e: ExprId, out: &mut Vec<Stmt>) -> ExprId {
    if e == NO_EXPR { return e; }
    let ex = f.exprs[e as usize].clone();
    if f.needs_short_circuit(e) {
        let ExprKind::Binary(op, a, b) = ex.kind else { unreachable!() };
        let a = value(f, a, out);
        let result = temp(f, a, out);
        let mut rhs = Vec::new();
        let b = value(f, b, &mut rhs);
        rhs.push(Stmt::Assign(result, b));
        let (yes, no) = if op == BinOp::LogicAnd { (rhs, Vec::new()) } else { (Vec::new(), rhs) };
        out.push(Stmt::If(result, yes, no));
        return result;
    }
    let kind = match ex.kind {
        ExprKind::Lit(_) | ExprKind::Nop | ExprKind::Error => return e,
        ExprKind::AddrOf(a) => {
            let a = place(f, a, out);
            return f.expr(ExprKind::AddrOf(a), ex.ty);
        }
        ExprKind::Param(p) if f.params[p as usize].inout => return e,
        ExprKind::Field(..) | ExprKind::Swizzle(..) | ExprKind::Index(..) | ExprKind::Deref(_) => {
            let p = place(f, e, out);
            return temp(f, p, out);
        }
        ExprKind::Unary(op, a) => ExprKind::Unary(op, value(f, a, out)),
        ExprKind::Convert(a) => ExprKind::Convert(value(f, a, out)),
        ExprKind::Bitcast(a) => ExprKind::Bitcast(value(f, a, out)),
        ExprKind::Binary(op, a, b) => {
            let a = value(f, a, out);
            ExprKind::Binary(op, a, value(f, b, out))
        }
        ExprKind::Construct(args) => ExprKind::Construct(args.into_iter().map(|a| value(f, a, out)).collect()),
        ExprKind::Call(fun, args) => ExprKind::Call(fun, args.into_iter().map(|a| value(f, a, out)).collect()),
        ExprKind::Builtin(op, args) => {
            let args = args.into_iter().enumerate().map(|(i, a)| {
                if i == 0 && matches!(op, Builtin::AtomicAdd | Builtin::CooperativeLoad | Builtin::CooperativeStore | Builtin::StorageTextureWrite) {
                    place(f, a, out)
                } else { value(f, a, out) }
            }).collect();
            ExprKind::Builtin(op, args)
        }
        ExprKind::Select(c, a, b) => {
            let c = value(f, c, out);
            let a = value(f, a, out);
            ExprKind::Select(c, a, value(f, b, out))
        }
        ExprKind::Sample(mut s) => {
            s.coord = value(f, s.coord, out);
            s.lod = value(f, s.lod, out);
            s.dx = value(f, s.dx, out);
            s.dy = value(f, s.dy, out);
            s.reference = value(f, s.reference, out);
            ExprKind::Sample(s)
        }
        ExprKind::TexLoad(t, c, layer, lod) => {
            let c = value(f, c, out);
            let layer = value(f, layer, out);
            ExprKind::TexLoad(t, c, layer, value(f, lod, out))
        }
        ExprKind::DepthClip(a, b, c) => {
            let a = value(f, a, out);
            let b = value(f, b, out);
            ExprKind::DepthClip(a, b, value(f, c, out))
        }
        kind => kind,
    };
    let e = f.expr(kind, ex.ty);
    if ex.ty == TY_VOID { e } else { temp(f, e, out) }
}

pub(crate) fn lower(f: &mut IrFunction, stmts: &[Stmt]) -> Vec<Stmt> {
    let mut out = Vec::new();
    for s in stmts {
        let s = match s {
            Stmt::Local(l, e) if contains(f, *e) => Stmt::Local(*l, value(f, *e, &mut out)),
            Stmt::Expr(e) if contains(f, *e) => Stmt::Expr(value(f, *e, &mut out)),
            Stmt::Return(e) if contains(f, *e) => Stmt::Return(value(f, *e, &mut out)),
            Stmt::Assign(p, e) | Stmt::CompoundAssign(p, _, e) if contains(f, *p) || contains(f, *e) => {
                let p = place(f, *p, &mut out);
                if let Stmt::CompoundAssign(_, op, _) = s {
                    let old = temp(f, p, &mut out);
                    let v = value(f, *e, &mut out);
                    let v = f.expr(ExprKind::Binary(*op, old, v), f.ty_of(p));
                    Stmt::Assign(p, v)
                } else {
                    Stmt::Assign(p, value(f, *e, &mut out))
                }
            }
            Stmt::If(c, a, b) => {
                let c = if contains(f, *c) { value(f, *c, &mut out) } else { *c };
                Stmt::If(c, lower(f, a), lower(f, b))
            }
            Stmt::For(l, a, b, body) => {
                let (a, b) = if contains(f, *a) || contains(f, *b) {
                    (value(f, *a, &mut out), value(f, *b, &mut out))
                } else { (*a, *b) };
                Stmt::For(*l, a, b, lower(f, body))
            }
            Stmt::Loop(body) => Stmt::Loop(lower(f, body)),
            _ => s.clone(),
        };
        out.push(s);
    }
    out
}
