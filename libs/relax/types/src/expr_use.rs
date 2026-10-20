//! Typed expression uses, following rustc_hir_typeck::expr_use_visitor.
//! A place is a source local plus typed projections; indices deliberately have
//! no identity. Rvalues are walked, but never mistaken for captured locals.
use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use crate::ast::*;
use crate::tcx::{EvalCx, Tcx};
use crate::program::DefKind;
use crate::typeck::{Body, Coerce, Res, DEREF_COERCE_KEY};
use crate::types::{TyId as Type, TyKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span { pub lo: u32, pub hi: u32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection { Deref, Field(u32, u32), Index, Subslice }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub root: u32,
    pub root_ty: Type,
    pub projections: Vec<(Projection, Type)>,
    pub expr: u32,
    pub span: Span,
}
impl Place {
    pub fn ty(&self) -> Type { self.projections.last().map_or(self.root_ty, |p| p.1) }
    pub fn before(&self, i: usize) -> Type { if i == 0 { self.root_ty } else { self.projections[i - 1].1 } }
    pub fn project(mut self, p: Projection, ty: Type) -> Self { self.projections.push((p, ty)); self }
    pub fn prefix_of(&self, other: &Self) -> bool {
        self.root == other.root && other.projections.starts_with(&self.projections)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BorrowKind { Imm, UniqueImm, Mut }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsumeMode { Copy, Move }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeReadCause { Let, Matched }
pub trait Delegate {
    fn consume(&mut self, place: Place, mode: ConsumeMode);
    fn borrow(&mut self, place: Place, kind: BorrowKind);
    fn mutate(&mut self, place: Place);
    /// `node`: the expression the read is for, rustc's `diag_expr_id` (the scrutinee or initializer).
    fn fake_read(&mut self, place: Place, cause: FakeReadCause, node: u32);
}
#[derive(Clone, Copy)]
enum Access { Consume, Borrow(BorrowKind), Mutate }

pub struct ExprUseVisitor<'a, 'tx, D> {
    pub tcx: &'a Tcx,
    pub cx: &'a EvalCx<'tx>,
    pub body: &'a Body,
    pub delegate: D,
    /// Missing semantic information must not silently become a shared borrow.
    pub errors: Vec<(u32, &'static str)>,
}
impl<D: Delegate> ExprUseVisitor<'_, '_, D> {
    fn ast(&self) -> &Ast { &self.cx.prog.files[self.body.file as usize].ast }
    fn span(&self, e: ExprId) -> Span { let e = self.ast().expr(e); Span { lo: e.lo, hi: e.hi } }
    fn local(&self, root: u32, e: ExprId) -> Place {
        Place { root, root_ty: self.body.locals[root as usize].ty, projections: Vec::new(), expr: e.0, span: self.span(e) }
    }
    fn consume_place(&mut self, p: Place) {
        match self.copy_type(p.ty()) {
            Some(copy) => self.delegate.consume(p, if copy { ConsumeMode::Copy } else { ConsumeMode::Move }),
            None => self.errors.push((p.expr, "unresolved Copy obligation")),
        }
    }
    /// Capture uses require a proven Copy goal; unresolved properties defer analysis.
    pub fn copy_type(&self, t: Type) -> Option<bool> {
        match self.tcx.resolved_kind(self.cx, t) {
            TyKind::Error | TyKind::Infer(_) => None,
            TyKind::Ref(true, _, _) => Some(false),
            TyKind::Tuple(ts) => {
                let mut copy = true;
                for t in self.tcx.tys.list(ts).iter().copied() { copy &= self.copy_type(t)?; }
                Some(copy)
            }
            TyKind::Array(t, _) => self.copy_type(t),
            TyKind::Adt(..) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) => {
                let Some(td) = self.tcx.lang_items.get(crate::lang_items::LangItem::Copy) else { return Some(false) };
                match self.tcx.evaluate_trait_modulo_regions(self.cx.prog, self.cx, self.body.def, td, t, &[]) {
                    r if r.must_apply_modulo_regions() => Some(true),
                    crate::next_solver::evaluate_obligation::EvaluationResult::EvaluatedToErr => Some(false),
                    // (rustc type_is_copy_modulo_regions: what is not known to be Copy is not; here the type must not
                    // be waiting for inference -- a goal waiting for a coroutine of the body never ends in it)
                    _ => {
                        let resolved = self.cx.infcx.map_or(t, |infcx| infcx.deeply_resolve_ignoring_regions(t));
                        if self.tcx.tys.has_infer(resolved) { None } else { Some(false) }
                    }
                }
            }
            // structural_traits.rs instantiate_constituent_tys_for_copy_clone_trait: a closure
            // is Copy when its tupled upvars are; coroutines are not (coroutine_clone is a
            // feature gate). The current owner reads its in-progress postorder result.
            TyKind::Closure(file, ce, args) => {
                if crate::upvar::is_capture_owner(self.tcx, self.body, file, args.parent_args) {
                    return self.body.min_captures.get(&ce).filter(|c| c.errors.is_empty()).map(|c| c.is_copy);
                }
                if self.tcx.is_coroutine(t) { return Some(false); }
                self.copy_type(args.tupled_upvars_ty())
            }
            _ => Some(self.tcx.type_is_copy_modulo_regions(self.cx.prog, self.cx, self.body.def, t)),
        }
    }
    fn access(&mut self, p: Place, a: Access) {
        match a {
            Access::Consume => self.consume_place(p),
            Access::Borrow(k) => self.delegate.borrow(p, k),
            Access::Mutate => self.delegate.mutate(p),
        }
    }
    pub fn consume_expr(&mut self, e: ExprId) { self.use_expr(e, Access::Consume); }
    fn use_expr(&mut self, e: ExprId, a: Access) {
        if let Some(p) = self.adjusted_place(e, matches!(a, Access::Mutate | Access::Borrow(BorrowKind::Mut))) { self.access(p, a); }
    }
    fn adjusted_place(&mut self, e: ExprId, mutable: bool) -> Option<Place> {
        let mut p = self.walk_place(e, mutable)?;
        // rustc walk_adjustment: Deref + Borrow(RawPtr) borrows the referent.
        // Cast operands go through this path just like implicit coercions.
        if let Some(Coerce::RefToRaw(m)) = self.body.coerce(e.0) {
            let t = self.tcx.builtin_deref(p.ty())?;
            p = p.project(Projection::Deref, t);
            self.delegate.borrow(p, if m { BorrowKind::Mut } else { BorrowKind::Imm });
            return None;
        }
        // coerce_to_ref (:406, :515): reborrow mutable references, while an
        // equal shared reference coercion is a no-op. Autoref is an rvalue.
        if let Some(m) = self.body.reborrow(e.0) {
            if let Some(t) = self.tcx.builtin_deref(p.ty()) {
                p = p.project(Projection::Deref, t);
                if let Some(Coerce::DerefRef(n)) = self.body.coerce(e.0) {
                    for i in 0..n { p = self.deref(p, e.0 | DEREF_COERCE_KEY, i, m)?; }
                }
                self.delegate.borrow(p, if m { BorrowKind::Mut } else { BorrowKind::Imm });
                return None;
            }
        }
        if self.body.coerce(e.0).is_some() { self.consume_place(p); None } else { Some(p) }
    }
    fn deref(&mut self, p: Place, key: u32, step: usize, mutable: bool) -> Option<Place> {
        if let Some(t) = self.tcx.builtin_deref(p.ty()) {
            return Some(p.project(Projection::Deref, t));
        }
        if self.body.ov_deref(key, step).is_some() {
            // place_op.rs:266 converts overloaded place operations in mutable
            // contexts. Selection must succeed before emitting its autoref.
            if mutable && !self.has_mutable_place_impl(crate::lang_items::LangItem::DerefMut, p.ty(), &[]) {
                self.errors.push((key, "unresolved DerefMut obligation"));
            } else { self.delegate.borrow(p, if mutable { BorrowKind::Mut } else { BorrowKind::Imm }); }
        } else { self.errors.push((key, "missing dereference adjustment")); }
        None
    }
    fn receiver(&mut self, e: ExprId, key: u32, n: usize, autoref: u8) {
        let Some(mut p) = self.walk_place(e, autoref == 2) else { return };
        for i in 0..n {
            let Some(q) = self.deref(p, key, i, autoref == 2) else { return };
            p = q;
        }
        self.access(p, match autoref { 1 => Access::Borrow(BorrowKind::Imm), 2 => Access::Borrow(BorrowKind::Mut), _ => Access::Consume });
    }
    /// The target of a recorded autoderef. Overloaded dereferences produce an
    /// rvalue reference from the selected method; its pointee is the adjusted
    /// callee type (rustc expr_use_visitor.rs:726-766).
    fn adjusted_deref_ty(&self, t: Type, key: u32, step: usize) -> Option<Type> {
        if let Some(target) = self.tcx.builtin_deref(t) { return Some(target); }
        let (method, args) = &self.body.ov_deref(key, step)?;
        let sig = self.tcx.fn_item_sig(self.cx.prog, *method)?;
        let ret = self.tcx.tys.subst(sig.ret, self.tcx.tys.list(*args));
        let ret = self.tcx.normalize_in_body(self.cx.prog, self.cx, self.body.def, ret);
        self.tcx.builtin_deref(ret)
    }
    /// Categorize the expression while visiting its evaluations. Field/deref
    /// bases are walked without consuming their containing local (:400-437).
    fn walk_expr(&mut self, e: ExprId) -> Option<Place> { self.walk_place(e, false) }
    fn has_mutable_place_impl(&self, item: crate::lang_items::LangItem, t: Type, args: &[Type]) -> bool {
        self.tcx.lang_items.get(item).is_some_and(|td| self.tcx.evaluate_trait_modulo_regions(self.cx.prog, self.cx, self.body.def, td, t, args).must_apply_modulo_regions())
    }
    fn walk_place(&mut self, e: ExprId, mutable: bool) -> Option<Place> {
        let kind = self.ast().expr(e).kind;
        match kind {
            ExprKind::Path(_) => if let Some(Res::Local(l)) = self.body.res.get(&e.0) { return Some(self.local(*l, e)); },
            ExprKind::Paren(x) => return self.walk_place(x, mutable),
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                let (n, fi) = self.body.field_idx.get(&e.0).map(|&(d, f)| (d as usize, f))?;
                let mut p = self.walk_place(x, mutable)?;
                for i in 0..n { p = self.deref(p, e.0, i, mutable)?; }
                p.expr = e.0; p.span = self.span(e);
                return Some(p.project(Projection::Field(0, fi), self.body.ty(e)));
            }
            ExprKind::Unary(UnOp::Deref, x) => {
                let p = self.walk_place(x, mutable)?;
                let mut p = self.deref(p, e.0, 0, mutable)?;
                p.expr = e.0; p.span = self.span(e);
                return Some(p);
            }
            ExprKind::Index(x, i) => {
                let p = self.walk_place(x, mutable);
                self.consume_expr(i);
                let mut p = p?;
                for j in 0..self.body.index_derefs(e.0) { p = self.deref(p, e.0, j, mutable)?; }
                if self.body.ov_index.contains_key(&e.0) {
                    if mutable && !self.has_mutable_place_impl(crate::lang_items::LangItem::IndexMut, p.ty(), &[self.body.ty(i)]) {
                        self.errors.push((e.0, "unresolved IndexMut obligation"));
                    } else { self.delegate.borrow(p, if mutable { BorrowKind::Mut } else { BorrowKind::Imm }); }
                    return None;
                }
                p.expr = e.0; p.span = self.span(e);
                return Some(p.project(Projection::Index, self.body.ty(e)));
            }
            ExprKind::AddrOf(_, m, x) => self.use_expr(x, Access::Borrow(if m { BorrowKind::Mut } else { BorrowKind::Imm })),
            ExprKind::Assign(x, y) => {
                if crate::lit::is_assignee_pattern(self.ast(), x) {
                    let source = self.adjusted_place(y, false);
                    self.walk_assignee(x, source, self.body.ty(y));
                } else { self.use_expr(x, Access::Mutate); self.consume_expr(y); }
            }
            ExprKind::AssignOp(_, x, y) => {
                // rustc_hir_typeck/src/op.rs:283-291: an overloaded `a op= b` adjusts the left
                // operand with `&mut` (the method takes `&mut self`), so the closure analysis
                // sees a mutable borrow of the place, not a by-value use
                if let Some(m) = self.body.binops.get(&e.0) { self.receiver(x, e.0, m.derefs as usize, 2); }
                else { self.use_expr(x, Access::Mutate); }
                self.consume_expr(y);
            }
            ExprKind::Binary(_, x, y) => {
                if let Some(m) = self.body.binops.get(&e.0) {
                    self.receiver(x, e.0, m.derefs as usize, m.autoref);
                    if m.autoref != 0 { self.use_expr(y, Access::Borrow(BorrowKind::Imm)); } else { self.consume_expr(y); }
                } else { self.consume_expr(x); self.consume_expr(y); }
            }
            ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Box(x) | ExprKind::Try(x) => self.consume_expr(x),
            ExprKind::MethodCall(recv, row) => {
                let args = self.ast().row(row).args;
                if let Some(m) = self.body.methods.get(&e.0) { self.receiver(recv, e.0, m.derefs as usize, m.autoref); }
                else { self.errors.push((e.0, "missing method callee")); }
                for x in self.ast().list(args).to_vec() { self.consume_expr(x); }
            }
            ExprKind::Call(f, args) => {
                let selection = self.body.call_traits.get(&e.0);
                let n = selection.map(|s| s.derefs).unwrap_or_else(|| self.body.call_derefs(e.0).unwrap_or(0));
                let mut t = Some(self.body.ty(f));
                for step in 0..n { t = t.and_then(|t| self.adjusted_deref_ty(t, e.0, step)); }
                match t.and_then(|t| self.body.call_kind(self.tcx, self.cx, e, t)) {
                    Some(crate::upvar::ClosureKind::FnOnce) if selection.is_some_and(|s| s.from_param) => {
                        // A generic FnOnce receiver is consumed at its definition.
                        // Monomorphizing F with a Copy closure cannot change that use.
                        let mut p = self.walk_place(f, false);
                        for step in 0..n { p = p.and_then(|p| self.deref(p, e.0, step, false)); }
                        if let Some(p) = p { self.delegate.consume(p, if selection.unwrap().by_move { ConsumeMode::Move } else { ConsumeMode::Copy }); }
                    }
                    Some(k) => self.receiver(f, e.0, n, match k { crate::upvar::ClosureKind::Fn => 1, crate::upvar::ClosureKind::FnMut => 2, crate::upvar::ClosureKind::FnOnce => 0 }),
                    None => self.errors.push((e.0, "unresolved call-trait adjustment")),
                }
                for x in self.ast().list(args).to_vec() { self.consume_expr(x); }
            }
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) | ExprKind::Loop(b, _) => self.walk_block(b),
            ExprKind::If(c, b, el) => { self.consume_expr(c); self.walk_block(b); if let Some(x) = el.get() { self.consume_expr(x); } }
            ExprKind::While(row) => { let crate::ast::WhileRow { cond: c, body: b, .. } = *self.ast().row(row); self.consume_expr(c); self.walk_block(b); }
            ExprKind::For(row) => { let crate::ast::ForRow { iter: x, body: b, .. } = *self.ast().row(row); self.consume_expr(x); self.walk_block(b); }
            ExprKind::Let(pat, x) => {
                if let Some(p) = self.adjusted_place(x, false) {
                    self.delegate.borrow(p.clone(), BorrowKind::Imm);
                    self.delegate.fake_read(p.clone(), FakeReadCause::Let, x.0);
                    self.walk_pat(pat, p, false);
                }
            },
            ExprKind::Match(x, arms) => {
                let place = self.adjusted_place(x, false);
                if let Some(p) = &place { self.delegate.fake_read(p.clone(), FakeReadCause::Matched, x.0); }
                for a in self.ast().list(arms).to_vec() {
                    if let Some(p) = &place { self.walk_pat(a.pat, p.clone(), a.guard.is_some()); }
                    if let Some(g) = a.guard { self.consume_expr(g); }
                    self.consume_expr(a.body);
                }
            }
            ExprKind::Closure(_) | ExprKind::Async(..) => {
                if let Some(caps) = self.body.min_captures.get(&e.0).cloned() {
                    for (p, cause, node) in caps.fake_reads { self.delegate.fake_read(p, cause, node); }
                    for c in caps.captures {
                        match c.info.kind { crate::upvar::CaptureKind::ByValue => self.consume_place(c.place), crate::upvar::CaptureKind::ByRef(k) => self.delegate.borrow(c.place, k) }
                    }
                    self.errors.extend(caps.errors);
                } else { self.errors.push((e.0, "nested closure analysis unavailable")); }
            }
            ExprKind::Tuple(v) | ExprKind::Array(v) => for x in self.ast().list(v).to_vec() { self.consume_expr(x); },
            ExprKind::Repeat(x, _) => self.consume_expr(x),
            ExprKind::Struct(row) => {
                let crate::ast::StructRow { fields: fs, base, .. } = *self.ast().row(row);
                let fs = self.ast().list(fs).to_vec();
                for f in &fs { self.consume_expr(f.expr); }
                if let Some(x) = base.get() {
                    if let Some(p) = self.walk_expr(x) {
                        let t = p.ty();
                        for fi in 0..self.tcx.place_n_fields(t, 0) as u32 {
                            let name = self.tcx.place_field_name(self.cx.prog, t, 0, fi);
                            let pf = &self.cx.prog.files[self.body.file as usize];
                            if fs.iter().any(|f| pf.text(f.name) == name) { continue; }
                            if let Some(ft) = self.tcx.place_field_ty(self.cx.prog, self.cx, t, 0, fi) { self.consume_place(p.clone().project(Projection::Field(0, fi), ft)); }
                        }
                    }
                }
            }
            ExprKind::Range(a, b, _) => for x in [a.get(), b.get()].into_iter().flatten() { self.consume_expr(x); },
            ExprKind::Return(x) | ExprKind::Break(_, x) | ExprKind::Yield(x) => if let Some(x) = x.get() { self.consume_expr(x); },
            ExprKind::Mac(m) => { let m = *self.ast().row(m); self.walk_macro(e, m) }
            ExprKind::Await(x) => self.consume_expr(x),
            ExprKind::Lit(_) | ExprKind::Continue(_) | ExprKind::ConstBlock(_) | ExprKind::Underscore => (),
        }
        None
    }
    // Destructuring assignment is a pattern match followed by assignments
    // (rustc_ast_lowering/expr.rs lower_expr_assign). Discards do not consume.
    fn walk_assignee(&mut self, e: ExprId, source: Option<Place>, ty: Type) {
        match self.ast().expr(e).kind {
            ExprKind::Paren(x) => self.walk_assignee(x, source, ty),
            ExprKind::Underscore | ExprKind::Range(OptExprId::NONE, OptExprId::NONE, false) => (),
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
                let xs = self.ast().list(xs).to_vec();
                let array = matches!(self.tcx.resolved_kind(self.cx, ty), TyKind::Array(..));
                let n = match self.tcx.resolved_kind(self.cx, ty) { TyKind::Array(_, n) => n.try_to_target_usize(&self.tcx.tys).expect("array length in expression use") as usize, _ => self.tcx.place_n_fields(ty, 0) };
                let rest = xs.iter().position(|x| matches!(self.ast().expr(*x).kind, ExprKind::Range(OptExprId::NONE, OptExprId::NONE, false)));
                for (i, x) in xs.iter().enumerate() {
                    if Some(i) == rest { continue; }
                    let fi = if rest.is_some_and(|r| i > r) { n - (xs.len() - i) } else { i } as u32;
                    let ft = match self.tcx.resolved_kind(self.cx, ty) { TyKind::Array(t, _) => Some(t), _ => self.tcx.place_field_ty(self.cx.prog, self.cx, ty, 0, fi) };
                    if let Some(ft) = ft { self.walk_assignee(*x, source.clone().map(|p| p.project(if array { Projection::Index } else { Projection::Field(0, fi) }, ft)), ft); }
                }
            }
            ExprKind::Struct(row) => {
                let fs = self.ast().row(row).fields;
                for f in self.ast().list(fs).to_vec() {
                    let name = self.cx.prog.files[self.body.file as usize].text(f.name);
                    for fi in 0..self.tcx.place_n_fields(ty, 0) as u32 {
                        if self.tcx.place_field_name(self.cx.prog, ty, 0, fi) == name {
                            if let Some(ft) = self.tcx.place_field_ty(self.cx.prog, self.cx, ty, 0, fi) { self.walk_assignee(f.expr, source.clone().map(|p| p.project(Projection::Field(0, fi), ft)), ft); }
                            break;
                        }
                    }
                }
            }
            ExprKind::Call(f, xs) => {
                let vi = match self.body.res.get(&f.0) { Some(Res::Def(d)) if self.cx.prog.def(*d).kind == DefKind::Variant => self.cx.prog.def(*d).sub, _ => 0 };
                let xs = self.ast().list(xs).to_vec();
                let n = self.tcx.place_n_fields(ty, vi);
                let rest = xs.iter().position(|x| matches!(self.ast().expr(*x).kind, ExprKind::Range(OptExprId::NONE, OptExprId::NONE, false)));
                for (i, x) in xs.iter().enumerate() {
                    if Some(i) == rest { continue; }
                    let fi = if rest.is_some_and(|r| i > r) { n - (xs.len() - i) } else { i } as u32;
                    if let Some(ft) = self.tcx.place_field_ty(self.cx.prog, self.cx, ty, vi, fi) { self.walk_assignee(*x, source.clone().map(|p| p.project(Projection::Field(vi, fi), ft)), ft); }
                }
            },
            _ => { if let Some(p) = source { self.consume_place(p); } self.use_expr(e, Access::Mutate); }
        }
    }
    fn walk_local(&mut self, pat: PatId, x: ExprId, cause: FakeReadCause, guard: bool) {
        if let Some(p) = self.adjusted_place(x, false) {
            self.delegate.fake_read(p.clone(), cause, x.0);
            self.walk_pat(pat, p, guard);
        }
    }
    pub(crate) fn walk_block(&mut self, b: BlockId) {
        for st in self.ast().stmts(b).to_vec() {
            match st {
                Stmt::Let(r) => {
                    let crate::ast::LetStmt { pat, init, else_, .. } = *self.ast().row(r);
                    if let Some(x) = init { self.walk_local(pat, x, if else_.is_some() { FakeReadCause::Matched } else { FakeReadCause::Let }, false); }
                    if let Some(b) = else_ { self.walk_block(b); }
                }
                Stmt::Expr(x, _) => self.consume_expr(x),
                Stmt::Attrs(r) => self.consume_expr(self.ast().row(r).expr),
                Stmt::Item(_) | Stmt::Empty => (),
            }
        }
    }
    /// walk_pat/cat_pattern (:833): the matched place, not the binding's
    /// reference type, determines the accessed projection.
    fn walk_pat(&mut self, pat: PatId, mut p: Place, guard: bool) {
        for _ in 0..self.body.pat_derefs.get(&pat.0).copied().unwrap_or(0) {
            if let Some(t) = self.tcx.builtin_deref(p.ty()) { p = p.project(Projection::Deref, t); }
        }
        if self.body.pat_discriminant_reads.contains_key(&pat.0) { self.delegate.borrow(p.clone(), BorrowKind::Imm); }
        let vi = match self.body.pat_res.get(&pat.0) { Some(Res::Def(d)) if self.cx.prog.def(*d).kind == DefKind::Variant => self.cx.prog.def(*d).sub, _ => 0 };
        match self.ast().pat(pat).clone() {
            Pat::Ident { by_ref, mutbl, sub, .. } => {
                if self.body.pat_local.contains_key(&pat.0) {
                    if guard { self.delegate.borrow(p.clone(), BorrowKind::Imm); }
                    let mode = self.body.pat_bind_ref.get(&pat.0).copied().unwrap_or(if by_ref { if mutbl { 2 } else { 1 } } else { 0 });
                    match mode { 1 => self.delegate.borrow(p.clone(), BorrowKind::Imm), 2 => self.delegate.borrow(p.clone(), BorrowKind::Mut), _ => self.consume_place(p.clone()) }
                } else if let Some(Res::Def(d)) = self.body.pat_res.get(&pat.0) {
                    if matches!(self.cx.prog.def(*d).kind, DefKind::Const | DefKind::AssocConst) { self.delegate.borrow(p.clone(), BorrowKind::Imm); }
                }
                if let Some(s) = sub { self.walk_pat(s, p, guard); }
            }
            Pat::Lit(_) | Pat::Range(..) => self.delegate.borrow(p, BorrowKind::Imm),
            Pat::Path(_) => if let Some(Res::Def(d)) = self.body.pat_res.get(&pat.0) {
                if matches!(self.cx.prog.def(*d).kind, DefKind::Const | DefKind::AssocConst) { self.delegate.borrow(p, BorrowKind::Imm); }
            },
            Pat::Tuple(v) | Pat::TupleStruct(_, v) => {
                let n = self.tcx.place_n_fields(p.ty(), vi);
                let ps = self.ast().list(v).to_vec();
                let rest = ps.iter().position(|p| matches!(self.ast().pat(*p), Pat::Rest));
                for (i, sp) in ps.iter().enumerate() {
                    if Some(i) == rest { continue; }
                    let fi = if rest.is_some_and(|r| i > r) { n - (ps.len() - i) } else { i } as u32;
                    if let Some(t) = self.tcx.place_field_ty(self.cx.prog, self.cx, p.ty(), vi, fi) { self.walk_pat(*sp, p.clone().project(Projection::Field(vi, fi), t), guard); }
                }
            }
            Pat::Struct(_, fs, _) => for f in self.ast().list(fs).to_vec() {
                let name = self.cx.prog.files[self.body.file as usize].text(f.name);
                for fi in 0..self.tcx.place_n_fields(p.ty(), vi) as u32 {
                    if self.tcx.place_field_name(self.cx.prog, p.ty(), vi, fi) == name {
                        if let Some(t) = self.tcx.place_field_ty(self.cx.prog, self.cx, p.ty(), vi, fi) { self.walk_pat(f.pat, p.clone().project(Projection::Field(vi, fi), t), guard); }
                        break;
                    }
                }
            },
            Pat::Slice(v) => {
                let ps = self.ast().list(v).to_vec();
                let rest_only = ps.len() == 1 && crate::typeck::is_rest_pat(self.ast(), ps[0]);
                if !rest_only && matches!(self.tcx.resolved_kind(self.cx, p.ty()), TyKind::Slice(_)) { self.delegate.borrow(p.clone(), BorrowKind::Imm); }
                if let TyKind::Slice(t) | TyKind::Array(t, _) = self.tcx.resolved_kind(self.cx, p.ty()) {
                    for sp in ps {
                        let rest = crate::typeck::is_rest_pat(self.ast(), sp);
                        self.walk_pat(sp, p.clone().project(if rest { Projection::Subslice } else { Projection::Index }, if rest { self.body.pty(sp) } else { t }), guard);
                    }
                }
            }
            Pat::Ref(_, s) | Pat::Box(s) => if let Some(t) = self.tcx.builtin_deref(p.ty()) { self.walk_pat(s, p.project(Projection::Deref, t), guard); },
            Pat::Paren(s) => self.walk_pat(s, p, guard),
            Pat::Or(v) => for s in self.ast().list(v).to_vec() { self.walk_pat(s, p.clone(), guard); },
            Pat::Wild | Pat::Rest => (),
            Pat::Mac(_) => self.errors.push((p.expr, "unexpanded pattern macro")),
        }
    }
    fn walk_macro(&mut self, e: ExprId, m: MacCall) {
        if m.args == NO_MAC_ARGS { return; }
        let pf = &self.cx.prog.files[self.body.file as usize];
        let name = self.ast().segs(m.path.segs).last().map(|s| pf.text(s.name)).unwrap_or("");
        let ma = *self.ast().mac_arg(m.args);
        let xs = self.ast().list(ma.exprs).to_vec();
        match name {
            "matches" => {
                if let (Some(p), Some(x)) = (ma.pat, xs.first()) { self.walk_local(p, *x, FakeReadCause::Matched, ma.guard.is_some()); }
                if let Some(g) = ma.guard { self.consume_expr(g); }
            }
            "assert" => {
                if let Some(x) = xs.first() { self.consume_expr(*x); }
                for x in xs.into_iter().skip(1) { self.use_expr(x, Access::Borrow(BorrowKind::Imm)); }
            }
            "assert_eq" | "assert_ne" | "format_args" | "format" | "print" | "println" | "eprint" | "eprintln" | "panic" | "unreachable" | "todo" | "unimplemented" => {
                for x in xs { self.use_expr(x, Access::Borrow(BorrowKind::Imm)); }
                let named: Vec<_> = self.body.fmt_named.iter().filter_map(|((ce, _), r)| if *ce == e.0 { if let Res::Local(l) = r { Some(*l) } else { None } } else { None }).collect();
                for l in named { self.delegate.borrow(self.local(l, e), BorrowKind::Imm); }
            }
            "cfg" | "debug_assert" | "debug_assert_eq" | "debug_assert_ne" => (),
            _ => self.errors.push((e.0, "unexpanded macro use semantics")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Events(Vec<(Place, &'static str)>);
    impl Delegate for Events {
        fn consume(&mut self, p: Place, m: ConsumeMode) { self.0.push((p, if m == ConsumeMode::Copy { "copy" } else { "move" })); }
        fn borrow(&mut self, p: Place, k: BorrowKind) { self.0.push((p, if k == BorrowKind::Mut { "mut" } else { "imm" })); }
        fn mutate(&mut self, p: Place) { self.0.push((p, "mutate")); }
        fn fake_read(&mut self, p: Place, _: FakeReadCause, _: u32) { self.0.push((p, "fake")); }
    }
    #[test]
    fn mutable_reference_consumption_depends_on_recorded_reborrow() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let mut prog = crate::program::Program::new(sess); let tcx = crate::tcx::Tcx::new();
        let pf = crate::parser::parse_owned(sess, b"fn f() { x; }".to_vec(), 2021).unwrap();
        let id = ExprId(pf.ast.exprs.iter().position(|e| matches!(e.kind, ExprKind::Path(_))).unwrap() as u32);
        let n = pf.ast.exprs.len(); prog.files.push(std::sync::Arc::new(pf));
        let t = tcx.tys.intern(TyKind::Ref(true, tcx.tys.bool_, crate::types::RegionId::ERASED));
        let mut b = Body::empty(crate::program::NO_DEF, vec![], 0, tcx.tys.unit);
        b.expr_ty = vec![t; n].into();
        b.locals.push(crate::typeck::Local { name: 0, ty: t, mutable: false, pat: 0 });
        b.res.insert(id.0, Res::Local(0));
        let state = crate::tcx::EvalState::new();
        let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
        for reborrow in [false, true] {
            if reborrow { b.adjustments.insert(id.0, crate::typeck::adjustments::Adjustments { operand: Default::default(), value: vec![crate::typeck::adjustments::Adjustment { kind: crate::typeck::adjustments::Adjust::Borrow(true), target: t, mutbl: false }].into() }); }
            let mut v = ExprUseVisitor { tcx: &tcx, cx: &cx, body: &b, delegate: Events::default(), errors: Vec::new() };
            v.consume_expr(id);
            assert!(v.errors.is_empty());
            assert_eq!(v.delegate.0.len(), 1);
            assert_eq!(v.delegate.0[0].1, if reborrow { "mut" } else { "move" });
            assert_eq!(v.delegate.0[0].0.projections.len(), usize::from(reborrow));
        }
    }
    #[test]
    fn projected_use_does_not_consume_the_base() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let mut prog = crate::program::Program::new(sess); let tcx = crate::tcx::Tcx::new();
        let pf = crate::parser::parse_owned(sess, b"fn f() { x.0; }".to_vec(), 2021).unwrap();
        let base = ExprId(pf.ast.exprs.iter().position(|e| matches!(e.kind, ExprKind::Path(_))).unwrap() as u32);
        let id = ExprId(pf.ast.exprs.iter().position(|e| matches!(e.kind, ExprKind::TupleField(..))).unwrap() as u32);
        let n = pf.ast.exprs.len(); prog.files.push(std::sync::Arc::new(pf));
        let t = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[tcx.tys.bool_])));
        let mut b = Body::empty(crate::program::NO_DEF, vec![], 0, tcx.tys.unit);
        b.expr_ty = vec![tcx.tys.bool_; n].into(); b.expr_ty[base.0 as usize] = t;
        b.locals.push(crate::typeck::Local { name: 0, ty: t, mutable: false, pat: 0 });
        b.res.insert(base.0, Res::Local(0)); b.field_idx.insert(id.0, (0, 0));
        let state = crate::tcx::EvalState::new();
        let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
        let mut v = ExprUseVisitor { tcx: &tcx, cx: &cx, body: &b, delegate: Events::default(), errors: Vec::new() };
        v.consume_expr(id);
        assert!(v.errors.is_empty()); assert_eq!(v.delegate.0.len(), 1);
        assert_eq!(v.delegate.0[0].1, "copy");
        assert_eq!(v.delegate.0[0].0.projections, vec![(Projection::Field(0, 0), tcx.tys.bool_)]);
    }
}
