//! `unconditional_recursion` (rustc_mir_transform check_call_recursion.rs). rustc searches the
//! built MIR's control-flow graph from the start block, cutting every edge that leaves a
//! recursive call (and the unwind edge of every call that can also return); the function is
//! reported when that search reaches no block that returns control (Return, Unreachable,
//! UnwindResume, Yield, ...) and no back edge, and found at least one recursive call.
//!
//! The port runs the same search over the structured syntax tree in MIR evaluation order:
//! `ev` returns whether control can fall through an expression while the search is still alive;
//! a recursive call ends the walk of its path, a `return`, a call that never returns (its unwind
//! edge reaches the caller), an empty `match` or a panic macro sets `escape`, and a loop whose
//! body or `continue` is reached sets `cycle` (the head is on the search stack).
//! Check-only.

use crate::ast::*;
use crate::lint::{self, Lint};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::typeck::{Body, Res};
use crate::types::{TyId, TyKind};

pub const UNCONDITIONAL_RECURSION: Lint = lint::lint("unconditional_recursion");

struct LoopCx {
    label: Option<String>,
    /// `loop` / `while` / `for` (an unlabeled `break` targets it); false: a labeled block
    is_loop: bool,
    brk: bool,
}

struct Rec<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    body: &'a Body,
    ast: &'a Ast,
    env: crate::tcx::TypingEnv,
    /// the function being checked
    caller: DefId,
    /// its trait generics when it is a trait method (`GenericArgs::identity_for_item(..)[..n]`)
    trait_args: u32,
    escape: bool,
    cycle: bool,
    rec_seen: bool,
    loops: Vec<LoopCx>,
}

/// Runs the check for one fn body.
pub fn check(prog: &Program, tcx: &Tcx, body: &Body, block: BlockId) {
    let d = prog.def(body.def);
    if !matches!(d.kind, DefKind::Fn | DefKind::AssocFn) { return; }
    let pf = &prog.files[body.file as usize];
    let ast = &pf.ast;
    let item = *ast.item(d.item);
    let ItemKind::Fn(sig, _, _) = item.kind else { return };
    // an `async fn`'s body only builds the coroutine
    if ast.fn_sig(sig).asyncness { return; }
    let trait_args = if d.kind == DefKind::AssocFn && d.parent != crate::program::NO_DEF && prog.def(d.parent).kind == DefKind::Trait {
        tcx.fn_sig(prog, body.def).map_or(0, |s| s.n_parent_generics)
    } else { 0 };
    let env = crate::pattern_analysis::body_cx(prog, tcx, body).typing_env();
    let mut r = Rec { prog, tcx, body, ast, env, caller: body.def, trait_args, escape: false, cycle: false, rec_seen: false, loops: Vec::new() };
    if r.block(block) { r.escape = true; }
    if r.escape || r.cycle || !r.rec_seen { return; }
    if lint::external(prog, body.file, item.lo) { return; }
    lint::emit(prog, body.def, &UNCONDITIONAL_RECURSION, body.file, item.lo, "function cannot return without recursing");
}

fn unparen(ast: &Ast, mut e: ExprId) -> ExprId {
    while let ExprKind::Paren(x) = ast.expr(e).kind { e = x; }
    e
}

impl<'a> Rec<'a> {
    fn kind(&self, t: TyId) -> TyKind { *self.tcx.tys.kind(t) }

    fn expr_ty(&self, e: ExprId) -> Option<TyId> {
        let i = self.body.expr_ix(e);
        if i < self.body.expr_ty.len() && self.body.expr_typed.get(i) { Some(self.body.expr_ty[i]) } else { None }
    }

    /// `CallRecursion::is_recursive_terminator` for a call of `callee` with `args` and `n` operands.
    fn is_recursive(&self, callee: DefId, args: crate::types::TyList, n: usize) -> bool {
        // the operand count of the callee must be the caller's argument count
        let caller_sig = match self.tcx.fn_sig(self.prog, self.caller) { Some(s) => s, None => return false };
        if n != caller_sig.params.len() { return false; }
        let args_v = self.tcx.tys.list(args).to_vec();
        let mut callee = callee;
        let mut call_args = args_v.clone();
        let d = self.prog.def(callee);
        if d.parent != crate::program::NO_DEF && self.prog.def(d.parent).kind == DefKind::Trait
            && !args_v.iter().any(|&a| self.tcx.tys.has_infer(a))
        {
            if let Ok(inst) = self.tcx.resolve_instance_raw(self.prog, self.env, callee, &args_v) {
                if let crate::instance::InstanceKind::Item(r) = inst.def {
                    callee = r;
                    call_args = inst.args.iter().filter_map(|g| match g { crate::consts::GenericArg::Type(t) => Some(*t), _ => None }).collect();
                }
            }
        }
        if callee != self.caller { return false; }
        // a trait method: the trait's generic arguments must be the identity
        for i in 0..self.trait_args as usize {
            match call_args.get(i).map(|&t| self.kind(t)) {
                Some(TyKind::Param(p)) if p as usize == i => {}
                _ => return false,
            }
        }
        true
    }

    fn rec_call(&mut self, callee: DefId, args: crate::types::TyList, n: usize) -> bool {
        if self.is_recursive(callee, args, n) { self.rec_seen = true; true } else { false }
    }

    /// An overloaded operator / deref / index call on `n` operands.
    fn op_call(&mut self, m: Option<(DefId, crate::types::TyList)>, n: usize) -> bool {
        match m {
            Some((d, a)) => self.rec_call(d, a, n),
            None => false,
        }
    }

    fn exprs(&mut self, l: List<ExprId>) -> bool {
        for &x in self.ast.list(l) { if !self.ev(x) { return false; } }
        true
    }

    /// The autoderef calls of an expression's adjustments (steps `0..n`).
    fn deref_calls(&mut self, keys: &[u32], n: usize) -> bool {
        for step in 0..n {
            if let Some((m, a)) = keys.iter().find_map(|k| self.body.ov_deref(*k, step)) {
                if self.rec_call(m, a, 1) { return false; }
            }
        }
        true
    }

    fn block(&mut self, b: BlockId) -> bool {
        let ast = self.ast;
        for s in ast.stmts(b) {
            match *s {
                Stmt::Let(r) => {
                    let l = *ast.row(r);
                    if let Some(x) = l.init {
                        if !self.ev(x) { return false; }
                    }
                    if let Some(e) = l.else_ { self.block(e); }
                }
                Stmt::Expr(e, _) => { if !self.ev(e) { return false; } }
                Stmt::Attrs(r) => { let a = *ast.row(r); if !self.ev(a.expr) { return false; } }
                Stmt::Item(_) | Stmt::Empty => {}
            }
        }
        true
    }

    /// Whether the condition can be true / can be false (let chains, `&&`, `||`, `!`).
    fn cond(&mut self, c: ExprId) -> (bool, bool) {
        let ast = self.ast;
        let c = unparen(ast, c);
        match ast.expr(c).kind {
            ExprKind::Binary(BinOp::And, a, b) if self.body.binops.get(&c.0).is_none() => {
                let (ta, fa) = self.cond(a);
                if !ta { return (false, fa); }
                let (tb, fb) = self.cond(b);
                (tb, fa || fb)
            }
            ExprKind::Binary(BinOp::Or, a, b) if self.body.binops.get(&c.0).is_none() => {
                let (ta, fa) = self.cond(a);
                if !fa { return (ta, false); }
                let (tb, fb) = self.cond(b);
                (ta || tb, fb)
            }
            ExprKind::Let(_, x) => { let ok = self.ev(x); (ok, ok) }
            _ => { let ok = self.ev(c); (ok, ok) }
        }
    }

    fn loop_target(&mut self, label: Option<String>) -> Option<&mut LoopCx> {
        match label {
            Some(l) => self.loops.iter_mut().rev().find(|c| c.label.as_deref() == Some(l.as_str())),
            None => self.loops.iter_mut().rev().find(|c| c.is_loop),
        }
    }

    fn label_text(&self, l: Option<Ident>) -> Option<String> {
        l.map(|i| self.prog.files[self.body.file as usize].text(i).to_string())
    }

    fn diverging(&mut self) -> bool { self.escape = true; false }

    /// Whether control can fall through `e`.
    fn ev(&mut self, e: ExprId) -> bool {
        let ast = self.ast;
        match ast.expr(e).kind {
            ExprKind::Lit(_) | ExprKind::Path(_) | ExprKind::Underscore | ExprKind::Closure(_) | ExprKind::Async(..) | ExprKind::ConstBlock(_) => true,
            ExprKind::Paren(x) | ExprKind::Cast(x, _) | ExprKind::AddrOf(_, _, x) | ExprKind::Box(x) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => self.ev(x),
            ExprKind::Repeat(x, _) => self.ev(x),
            ExprKind::Unary(op, x) => {
                if !self.ev(x) { return false; }
                if matches!(op, UnOp::Deref) {
                    if !self.deref_calls(&[e.0], 1) { return false; }
                } else if let Some(m) = self.body.binops.get(&e.0).copied() {
                    if self.rec_call(m.def, m.args, 1) { return false; }
                }
                true
            }
            ExprKind::Binary(op, a, b) => {
                let overloaded = self.body.binops.get(&e.0).copied();
                if overloaded.is_none() && matches!(op, BinOp::And | BinOp::Or) {
                    if !self.ev(a) { return false; }
                    self.ev(b);
                    return true;
                }
                if !self.ev(a) || !self.ev(b) { return false; }
                if let Some(m) = overloaded { if self.rec_call(m.def, m.args, 2) { return false; } }
                true
            }
            ExprKind::Assign(l, r) => self.ev(r) && self.ev(l),
            ExprKind::AssignOp(_, l, r) => {
                if !self.ev(l) || !self.ev(r) { return false; }
                if let Some(m) = self.body.binops.get(&e.0).copied() { if self.rec_call(m.def, m.args, 2) { return false; } }
                true
            }
            ExprKind::Index(b, i) => {
                if !self.ev(b) || !self.ev(i) { return false; }
                let m = self.body.ov_index.get(&e.0).copied();
                !self.op_call(m, 2)
            }
            ExprKind::Call(f, args) => {
                if !self.ev(f) || !self.exprs(args) { return false; }
                let n = ast.list(args).len();
                let fu = unparen(ast, f);
                if matches!(ast.expr(fu).kind, ExprKind::Path(_)) {
                    if let Some(Res::Def(d)) = self.body.res.get(&fu.0).copied() {
                        if matches!(self.prog.def(d).kind, DefKind::Fn | DefKind::AssocFn) {
                            let a = self.body.res_args.get(&fu.0).copied().unwrap_or(crate::types::TyList::EMPTY);
                            if self.rec_call(d, a, n) { return false; }
                            if self.tcx.fn_item_sig(self.prog, d).is_some_and(|s| matches!(self.kind(s.ret), TyKind::Never)) { return self.diverging(); }
                        }
                    }
                }
                if self.expr_ty(e).is_some_and(|t| matches!(self.kind(t), TyKind::Never)) { return self.diverging(); }
                true
            }
            ExprKind::MethodCall(r, row) => {
                if !self.ev(r) { return false; }
                if let Some(m) = self.body.methods.get(&e.0).copied() {
                    if !self.deref_calls(&[e.0, r.0, unparen(ast, r).0], m.derefs as usize) { return false; }
                }
                let args = ast.row(row).args;
                if !self.exprs(args) { return false; }
                if let Some(m) = self.body.methods.get(&e.0).copied() {
                    if self.rec_call(m.def, m.args, ast.list(args).len() + 1) { return false; }
                    if self.tcx.fn_item_sig(self.prog, m.def).is_some_and(|s| matches!(self.kind(s.ret), TyKind::Never)) { return self.diverging(); }
                }
                if self.expr_ty(e).is_some_and(|t| matches!(self.kind(t), TyKind::Never)) { return self.diverging(); }
                true
            }
            ExprKind::Block(b, label) => {
                let label = self.label_text(label.get());
                if label.is_some() {
                    self.loops.push(LoopCx { label, is_loop: false, brk: false });
                    let ok = self.block(b);
                    let c = self.loops.pop().unwrap();
                    ok || c.brk
                } else { self.block(b) }
            }
            ExprKind::Unsafe(b) => self.block(b),
            ExprKind::If(c, then, els) => {
                let (t, f) = self.cond(c);
                let a = t && self.block(then);
                let b = f && match els.get() { Some(x) => self.ev(x), None => true };
                a || b
            }
            ExprKind::Let(_, x) => self.ev(x),
            ExprKind::Match(x, arms) => {
                if !self.ev(x) { return false; }
                let arms = ast.list(arms);
                if arms.is_empty() { return self.diverging(); }
                let mut any = false;
                for a in arms {
                    if let Some(g) = a.guard {
                        let (t, _) = self.cond(g);
                        if !t { continue; }
                    }
                    if self.ev(a.body) { any = true; }
                }
                any
            }
            ExprKind::Loop(b, label) => {
                let label = self.label_text(label.get());
                self.loops.push(LoopCx { label, is_loop: true, brk: false });
                if self.block(b) { self.cycle = true; }
                self.loops.pop().unwrap().brk
            }
            ExprKind::While(row) => {
                let w = *ast.row(row);
                let label = self.label_text(w.label);
                self.loops.push(LoopCx { label, is_loop: true, brk: false });
                let (t, f) = self.cond(w.cond);
                if t && self.block(w.body) { self.cycle = true; }
                let c = self.loops.pop().unwrap();
                f || c.brk
            }
            ExprKind::For(row) => {
                let f = *ast.row(row);
                if !self.ev(f.iter) { return false; }
                if let Some(&(d, a, _)) = self.body.for_into.get(&f.iter.0).or_else(|| self.body.for_into.get(&e.0)) {
                    if self.rec_call(d, a, 1) { return false; }
                }
                let label = self.label_text(f.label);
                self.loops.push(LoopCx { label, is_loop: true, brk: false });
                let mut next_ok = true;
                if let Some(&(d, a)) = self.body.for_next.get(&f.iter.0).or_else(|| self.body.for_next.get(&e.0)) {
                    if self.rec_call(d, a, 1) { next_ok = false; }
                }
                if next_ok && self.block(f.body) { self.cycle = true; }
                let c = self.loops.pop().unwrap();
                next_ok || c.brk
            }
            ExprKind::Break(label, val) => {
                if let Some(v) = val.get() { if !self.ev(v) { return false; } }
                let l = self.label_text(label.get());
                if let Some(c) = self.loop_target(l) { c.brk = true; }
                false
            }
            ExprKind::Continue(_) => { self.cycle = true; false }
            ExprKind::Return(x) => {
                if let Some(v) = x.get() { if !self.ev(v) { return false; } }
                self.diverging()
            }
            ExprKind::Yield(x) => {
                if let Some(v) = x.get() { if !self.ev(v) { return false; } }
                self.diverging()
            }
            ExprKind::Try(x) => {
                if !self.ev(x) { return false; }
                // the residual arm returns
                self.escape = true;
                true
            }
            ExprKind::Await(x) => self.ev(x),
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => self.exprs(xs),
            ExprKind::Struct(row) => {
                let s = *ast.row(row);
                for f in ast.list(s.fields) { if !self.ev(f.expr) { return false; } }
                match s.base.get() { Some(b) => self.ev(b), None => true }
            }
            ExprKind::Range(a, b, _) => {
                if let Some(x) = a.get() { if !self.ev(x) { return false; } }
                match b.get() { Some(x) => self.ev(x), None => true }
            }
            ExprKind::Mac(m) => {
                let m = *ast.row(m);
                if m.args != NO_MAC_ARGS {
                    let args = *ast.mac_arg(m.args);
                    if !self.exprs(args.exprs) { return false; }
                }
                let name = ast.segs(m.path.segs).last().map(|s| self.prog.files[self.body.file as usize].text(s.name).to_string()).unwrap_or_default();
                match name.as_str() {
                    "panic" | "unreachable" | "todo" | "unimplemented" => self.diverging(),
                    // the failing branch panics
                    "assert" | "assert_eq" | "assert_ne" | "debug_assert" | "debug_assert_eq" | "debug_assert_ne" => { self.escape = true; true }
                    _ => true,
                }
            }
        }
    }
}
