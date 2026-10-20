//! rustc_mir_build/src/thir/cx/expr.rs: expressions, with their adjustments made explicit.
use super::{Cx, ExprKind};
use crate::ast::{self, ExprKind as A, OptExprId};
use crate::consts::ScalarInt;
use crate::lang_items::LangItem;
use crate::mir::build::thir::{AdtExpr, AdtExprBase, AssignOp, ClosureExpr, Expr, ExprId, FieldExpr, FruInfo, LocalVarId, LogicalOp, MatchSource, Arm, ArmId};
use crate::mir::build::{BuildResult, UnsupportedReason};
use crate::mir::{BinOp as MBinOp, BorrowKind, FakeReadCause, FieldIdx, MutBorrowKind, Mutability, PointerCoercion, Span, UnOp as MUnOp, VariantIdx};
use crate::program::{DefId, DefKind};
use crate::region_scope::{Node, ScopeData};
use crate::typeck::adjustments::{Adjust, Adjustment};
use crate::typeck::user_types::UserTypeKey;
use crate::typeck::{Coerce, Res};
use crate::types::user_type::{CanonicalUserType, UserTypeKind};
use crate::types::{Safety, TyId, TyKind};

type UserTy = Option<Box<CanonicalUserType>>;

fn bin_op(op: ast::BinOp) -> Option<MBinOp> {
    use ast::BinOp::*;
    Some(match op {
        Add => MBinOp::Add,
        Sub => MBinOp::Sub,
        Mul => MBinOp::Mul,
        Div => MBinOp::Div,
        Rem => MBinOp::Rem,
        BitXor => MBinOp::BitXor,
        BitAnd => MBinOp::BitAnd,
        BitOr => MBinOp::BitOr,
        Shl => MBinOp::Shl,
        Shr => MBinOp::Shr,
        Eq => MBinOp::Eq,
        Lt => MBinOp::Lt,
        Le => MBinOp::Le,
        Ne => MBinOp::Ne,
        Ge => MBinOp::Ge,
        Gt => MBinOp::Gt,
        And | Or => return None,
    })
}

fn mut_borrow(two_phase: bool) -> BorrowKind {
    BorrowKind::Mut { kind: if two_phase { MutBorrowKind::TwoPhaseBorrow } else { MutBorrowKind::Default } }
}

impl Cx<'_> {
    /// Create a THIR expression for the given expression. This expands all adjustments and directly
    /// adds the type information from the typeck results.
    pub fn mirror_expr(&mut self, e: ast::ExprId) -> BuildResult<ExprId> {
        let two_phase = std::mem::take(&mut self.two_phase);
        let span = self.espan(e);
        let expr_scope = self.node_scope(Node::Expr(e.0), span)?;
        let mut expr = self.make_mirror_unadjusted(e)?;
        // Now apply adjustments, if any.
        for adjustment in self.value_adjustments(e, expr.ty) {
            expr = self.apply_adjustment(e, expr, adjustment, two_phase)?;
        }
        // Finally, wrap this up in the expr's scope.
        let (ty, temp_scope, bid) = (expr.ty, self.temp_scope(e), self.bid_scope(e));
        let value = self.push(expr);
        Ok(self.push(Expr { temp_scope, bid, ty, span, kind: ExprKind::Scope { region_scope: expr_scope, node: e.0, value } }))
    }

    pub(super) fn mirror_exprs(&mut self, list: &[ast::ExprId]) -> BuildResult<Box<[ExprId]>> {
        list.iter().map(|&x| self.mirror_expr(x)).collect::<BuildResult<Vec<_>>>().map(Vec::into_boxed_slice)
    }

    /// A call argument: coercion reborrows of it are two-phase.
    pub(super) fn mirror_arg(&mut self, e: ast::ExprId) -> BuildResult<ExprId> {
        self.two_phase = true;
        self.mirror_expr(e)
    }

    fn user_ty_of(&self, key: UserTypeKey) -> UserTy {
        self.tb().user_provided_types.get(&key).map(|u| Box::new((**u).clone()))
    }

    // ---------------------------------------------------------------- adjustments

    /// The value-site adjustments of `e`: typeck stores a reference coercion without its leading
    /// builtin `Deref(None)`; insert it as rustc has it.
    fn value_adjustments(&self, e: ast::ExprId, source_ty: TyId) -> Vec<Adjustment> {
        let Some(adj) = self.tb().adjustments(e.0) else { return Vec::new() };
        let mut list: Vec<Adjustment> = adj.value.iter().copied().collect();
        if list.iter().any(|a| matches!(a.kind, Adjust::Borrow(_))) {
            if let TyKind::Ref(_, pointee, _) = *self.tcx().tys.kind(source_ty) {
                list.insert(0, Adjustment { kind: Adjust::Deref(None), target: pointee, mutbl: false });
            }
        }
        list
    }

    fn is_box_ty(&self, ty: TyId) -> bool {
        matches!(self.tcx().tys.kind(ty), TyKind::Adt(def, ..) if self.tcx().adts.get(def).is_some_and(|a| a.is_box()))
    }

    fn is_mut_method(&self, def: DefId) -> bool {
        matches!(self.prog().syms.str(self.prog().def(def).name), "deref_mut" | "index_mut")
    }

    fn apply_adjustment(&mut self, e: ast::ExprId, expr: Expr, adjustment: Adjustment, two_phase: bool) -> BuildResult<Expr> {
        let (temp_scope, bid, span) = (expr.temp_scope, expr.bid, expr.span);
        let target = adjustment.target;
        let tcx = self.tcx();
        let kind = match adjustment.kind {
            Adjust::Coerce(coerce) => match coerce {
                Coerce::Never if matches!(tcx.tys.kind(target), TyKind::Never) => return Ok(expr),
                Coerce::Never => ExprKind::NeverToAny { source: self.push(expr) },
                Coerce::ReifyFn | Coerce::ClosureFn => {
                    let safety = match *tcx.tys.kind(target) {
                        TyKind::FnPtr(_, _, header, _) => header.safety,
                        _ => Safety::Safe,
                    };
                    let cast = if coerce == Coerce::ReifyFn { PointerCoercion::ReifyFnPointer(safety) } else { PointerCoercion::ClosureFnPointer(safety) };
                    ExprKind::PointerCoercion { cast, source: self.push(expr), is_from_as_cast: self.cast_source == Some(e) }
                }
                Coerce::UnsafeFn => ExprKind::PointerCoercion { cast: PointerCoercion::UnsafeFnPointer, source: self.push(expr), is_from_as_cast: self.cast_source == Some(e) },
                Coerce::MutToConst => ExprKind::PointerCoercion { cast: PointerCoercion::MutToConstPointer, source: self.push(expr), is_from_as_cast: self.cast_source == Some(e) },
                Coerce::Unsize | Coerce::ToDyn(..) | Coerce::Upcast(..) => {
                    ExprKind::PointerCoercion { cast: PointerCoercion::Unsize, source: self.push(expr), is_from_as_cast: self.cast_source == Some(e) }
                }
                Coerce::RefToRaw(mutable) => {
                    // `&T as *const T`: [Deref(None), Borrow(RawPtr)]
                    let TyKind::Ref(_, pointee, _) = *tcx.tys.kind(expr.ty) else { return self.unresolved(span, "RefToRaw of a non-reference") };
                    let place = Expr { kind: ExprKind::Deref { arg: self.push(expr) }, ty: pointee, temp_scope, bid, span };
                    ExprKind::RawBorrow { mutability: if mutable { Mutability::Mut } else { Mutability::Not }, arg: self.push(place) }
                }
                Coerce::DerefRef(_) => return self.unresolved(span, "DerefRef is a view, not a step"),
            },
            Adjust::Deref(None) => ExprKind::Deref { arg: self.push(expr) },
            // a `Box` is dereferenced by the compiler, not through `Deref::deref` (rustc
            // `builtin_deref(explicit = false)`): typeck records its step as overloaded for codegen
            Adjust::Deref(Some(_)) if self.is_box_ty(expr.ty) => ExprKind::Deref { arg: self.push(expr) },
            Adjust::Deref(Some((def, args))) => {
                // `*Deref::deref(&expr)`
                // rustc `OverloadedDeref.mutbl`: the step calls `DerefMut::deref_mut`
                let mutable = adjustment.mutbl;
                let def = if mutable { self.tcx().mutable_place_op(self.prog(), def).unwrap_or(def) } else { def };
                let callee_ty = self.fn_def_ty(def, tcx.tys.list(args));
                let ref_ty = self.ref_ty(mutable, expr.ty);
                let borrow_kind = if mutable { mut_borrow(false) } else { BorrowKind::Shared };
                let borrow = Expr { kind: ExprKind::Borrow { borrow_kind, arg: self.push(expr) }, ty: ref_ty, temp_scope, bid, span };
                let arg = self.push(borrow);
                return Ok(self.overloaded_place(e, (temp_scope, bid), target, callee_ty, Box::new([arg]), span)?);
            }
            Adjust::Borrow(mutable) => {
                let borrow_kind = if mutable { mut_borrow(two_phase) } else { BorrowKind::Shared };
                ExprKind::Borrow { borrow_kind, arg: self.push(expr) }
            }
        };
        Ok(Expr { kind, ty: target, temp_scope, bid, span })
    }

    /// The operand-site adjustments of `node` (the autoderef of a receiver, base or callee).
    pub(super) fn apply_operand_adjustments(&mut self, node: ast::ExprId, mut expr: Expr) -> BuildResult<Expr> {
        let steps: Vec<Adjustment> = self.tb().adjustments(node.0).map(|a| a.operand.iter().copied().collect()).unwrap_or_default();
        for step in steps {
            expr = self.apply_adjustment(node, expr, step, false)?;
        }
        Ok(expr)
    }

    /// For an overloaded `*x` or `x[y]` of type T the method call returns an `&T` and we add the deref
    /// so that the types line up (the operators denote places): `*foo()`.
    fn overloaded_place(&mut self, e: ast::ExprId, (temp_scope, bid): (Option<crate::region_scope::ScopeId>, Option<crate::region_scope::ScopeId>), place_ty: TyId, callee_ty: TyId, args: Box<[ExprId]>, span: Span) -> BuildResult<Expr> {
        let tcx = self.tcx();
        let receiver_ty = self.thir.exprs[args[0].0 as usize].ty;
        let TyKind::Ref(mutable, _, region) = *tcx.tys.kind(receiver_ty) else { return self.unresolved(span, "overloaded place receiver is not a reference") };
        let ref_ty = tcx.tys.intern(TyKind::Ref(mutable, place_ty, region));
        let fun = self.push(Expr { kind: ExprKind::ZstLiteral { user_ty: None }, ty: callee_ty, temp_scope, bid, span });
        let call = self.push(Expr { kind: ExprKind::Call { ty: callee_ty, fun, args, from_hir_call: false, fn_span: span }, ty: ref_ty, temp_scope, bid, span });
        let _ = e;
        Ok(Expr { kind: ExprKind::Deref { arg: call }, ty: place_ty, temp_scope, bid, span })
    }

    /// A call of an overloaded operator: the trait method applied to the operands.
    fn overloaded_operator(&mut self, e: ast::ExprId, m: crate::typeck::MethodRes, args: Box<[ExprId]>, ty: TyId) -> BuildResult<Expr> {
        let span = self.espan(e);
        let callee_ty = self.fn_def_ty(m.def, self.tcx().tys.list(m.args));
        let fun = self.push(self.mk(e, ExprKind::ZstLiteral { user_ty: None }, callee_ty));
        Ok(self.mk_at(e, span, ExprKind::Call { ty: callee_ty, fun, args, from_hir_call: false, fn_span: span }, ty))
    }

    /// A binary operator on primitives where an operand is a reference (`1 << i` with `i: &usize`,
    /// `x += &y`): typeck treats it as builtin, but rustc only has the builtin operation on two
    /// primitives, so this is the operator trait's method (`<i32 as Shl<&usize>>::shl`).
    fn ref_operand_operator(&mut self, e: ast::ExprId, op: ast::BinOp, l: ExprId, r: ExprId, assign: bool, ty: TyId) -> BuildResult<Option<Expr>> {
        use ast::BinOp as B;
        let tcx = self.tcx();
        let (lt, rt) = (self.thir.exprs[l.0 as usize].ty, self.thir.exprs[r.0 as usize].ty);
        if !matches!(tcx.tys.kind(lt), TyKind::Ref(..)) && !matches!(tcx.tys.kind(rt), TyKind::Ref(..)) {
            return Ok(None);
        }
        let span = self.espan(e);
        let (item, name, compare) = match (op, assign) {
            (B::Add, false) => (LangItem::Add, "add", false),
            (B::Sub, false) => (LangItem::Sub, "sub", false),
            (B::Mul, false) => (LangItem::Mul, "mul", false),
            (B::Div, false) => (LangItem::Div, "div", false),
            (B::Rem, false) => (LangItem::Rem, "rem", false),
            (B::BitXor, false) => (LangItem::BitXor, "bitxor", false),
            (B::BitAnd, false) => (LangItem::BitAnd, "bitand", false),
            (B::BitOr, false) => (LangItem::BitOr, "bitor", false),
            (B::Shl, false) => (LangItem::Shl, "shl", false),
            (B::Shr, false) => (LangItem::Shr, "shr", false),
            (B::Add, true) => (LangItem::AddAssign, "add_assign", false),
            (B::Sub, true) => (LangItem::SubAssign, "sub_assign", false),
            (B::Mul, true) => (LangItem::MulAssign, "mul_assign", false),
            (B::Div, true) => (LangItem::DivAssign, "div_assign", false),
            (B::Rem, true) => (LangItem::RemAssign, "rem_assign", false),
            (B::BitXor, true) => (LangItem::BitXorAssign, "bitxor_assign", false),
            (B::BitAnd, true) => (LangItem::BitAndAssign, "bitand_assign", false),
            (B::BitOr, true) => (LangItem::BitOrAssign, "bitor_assign", false),
            (B::Shl, true) => (LangItem::ShlAssign, "shl_assign", false),
            (B::Shr, true) => (LangItem::ShrAssign, "shr_assign", false),
            (B::Eq, _) => (LangItem::PartialEq, "eq", true),
            (B::Ne, _) => (LangItem::PartialEq, "ne", true),
            (B::Lt, _) => (LangItem::PartialOrd, "lt", true),
            (B::Le, _) => (LangItem::PartialOrd, "le", true),
            (B::Gt, _) => (LangItem::PartialOrd, "gt", true),
            (B::Ge, _) => (LangItem::PartialOrd, "ge", true),
            _ => return Ok(None),
        };
        let Some(def) = self.trait_method(item, name) else { return self.unresolved(span, "operator trait method") };
        let callee_ty = self.fn_def_ty(def, &[lt, rt]);
        let fun = self.push(self.mk(e, ExprKind::ZstLiteral { user_ty: None }, callee_ty));
        let args: Box<[ExprId]> = if compare {
            Box::new([self.autoref(e, l, false), self.autoref(e, r, false)])
        } else if assign {
            Box::new([self.autoref_two_phase(e, l), r])
        } else {
            Box::new([l, r])
        };
        Ok(Some(self.mk_at(e, span, ExprKind::Call { ty: callee_ty, fun, args, from_hir_call: false, fn_span: span }, ty)))
    }

    /// `&expr` / `&mut expr` of an already built operand (an autoref).
    fn autoref(&mut self, e: ast::ExprId, expr: ExprId, mutable: bool) -> ExprId {
        let operand_ty = self.thir.exprs[expr.0 as usize].ty;
        let ty = self.ref_ty(mutable, operand_ty);
        let borrow_kind = if mutable { mut_borrow(false) } else { BorrowKind::Shared };
        let span = self.thir.exprs[expr.0 as usize].span;
        self.push(self.mk_at(e, span, ExprKind::Borrow { borrow_kind, arg: expr }, ty))
    }

    // ---------------------------------------------------------------- unadjusted

    fn make_mirror_unadjusted(&mut self, e: ast::ExprId) -> BuildResult<Expr> {
        let ast = self.ast();
        let tb = self.tb();
        let tcx = self.tcx();
        let span = self.espan(e);
        let expr_ty = self.ety(e)?;
        let kind = match ast.expr(e).kind {
            A::MethodCall(recv, row) => return self.method_call(e, recv, *ast.row(row), expr_ty),
            A::Call(fun, args) => return self.call(e, fun, ast.list(args), expr_ty),
            A::Paren(x) => {
                // parentheses do not exist in HIR: the inner expression, with the paren node's adjustments
                let inner = self.mirror_expr(x)?;
                return Ok(self.thir.exprs[inner.0 as usize].clone());
            }
            A::AddrOf(raw, mutable, x) => {
                let arg = self.mirror_expr(x)?;
                if raw {
                    ExprKind::RawBorrow { mutability: if mutable { Mutability::Mut } else { Mutability::Not }, arg }
                } else {
                    ExprKind::Borrow { borrow_kind: if mutable { mut_borrow(false) } else { BorrowKind::Shared }, arg }
                }
            }
            A::Block(b, label) => {
                let targeted = label.get().is_some();
                if let Some(label) = label.get() {
                    let scope = self.node_scope(Node::Block(b.0), span)?;
                    self.breakables.push(super::Breakable { label: Some(label), scope, is_block: true });
                }
                let block = self.mirror_block_with(b, super::BlockSafety::Safe, targeted);
                if label.get().is_some() {
                    self.breakables.pop();
                }
                ExprKind::Block { block: block? }
            }
            A::Unsafe(b) => ExprKind::Block { block: self.mirror_block_with(b, super::BlockSafety::ExplicitUnsafe(b.0), false)? },
            A::Assign(lhs, rhs) => {
                if crate::lit::is_assignee_pattern(ast, lhs) {
                    return self.destructuring_assign(e, lhs, rhs, expr_ty);
                }
                ExprKind::Assign { lhs: self.mirror_expr(lhs)?, rhs: self.mirror_expr(rhs)? }
            }
            A::AssignOp(op, lhs, rhs) => {
                if let Some(m) = tb.binops.get(&e.0).copied() {
                    // `OpAssign::op_assign(&mut lhs, rhs)`
                    let lhs = self.mirror_expr(lhs)?;
                    let lhs = self.autoref_two_phase(e, lhs);
                    let rhs = self.mirror_arg(rhs)?;
                    return self.overloaded_operator(e, m, Box::new([lhs, rhs]), expr_ty);
                }
                let Some(mop) = bin_op(op) else { return self.unresolved(span, "assignment operator") };
                let (l, r) = (self.mirror_expr(lhs)?, self.mirror_expr(rhs)?);
                if let Some(call) = self.ref_operand_operator(e, op, l, r, true, expr_ty)? {
                    return Ok(call);
                }
                ExprKind::AssignOp { op: AssignOp(mop), lhs: l, rhs: r }
            }
            A::Lit(_) => ExprKind::Literal { expr: e, neg: false },
            A::Binary(op, lhs, rhs) => {
                if let Some(m) = tb.binops.get(&e.0).copied() {
                    let mut l = self.mirror_expr(lhs)?;
                    let mut r = self.mirror_expr(rhs)?;
                    if m.autoref == 1 {
                        // comparisons: `PartialEq::eq(&lhs, &rhs)`
                        l = self.autoref(e, l, false);
                        r = self.autoref(e, r, false);
                    }
                    return self.overloaded_operator(e, m, Box::new([l, r]), expr_ty);
                }
                match op {
                    ast::BinOp::And => ExprKind::LogicalOp { op: LogicalOp::And, lhs: self.mirror_expr(lhs)?, rhs: self.mirror_expr(rhs)? },
                    ast::BinOp::Or => ExprKind::LogicalOp { op: LogicalOp::Or, lhs: self.mirror_expr(lhs)?, rhs: self.mirror_expr(rhs)? },
                    _ => {
                        let Some(mop) = bin_op(op) else { return self.unresolved(span, "binary operator") };
                        let (l, r) = (self.mirror_expr(lhs)?, self.mirror_expr(rhs)?);
                        if let Some(call) = self.ref_operand_operator(e, op, l, r, false, expr_ty)? {
                            return Ok(call);
                        }
                        ExprKind::Binary { op: mop, lhs: l, rhs: r }
                    }
                }
            }
            A::Index(lhs, index) => {
                if let Some(&(def, args)) = tb.mutable_index.get(&e.0).or_else(|| tb.ov_index.get(&e.0)).or_else(|| tb.std_index.get(&e.0)) {
                    // `*Index::index(&lhs, index)`
                    let base = self.mirror_expr(lhs)?;
                    let base = self.apply_operand_adjustments_id(e, base)?;
                    let base = self.autoref(e, base, self.is_mut_method(def));
                    // rustc try_index_step: an array indexed through an impl for the slice is unsized first
                    // (`[Adjust::Borrow, Adjust::Pointer(Unsize)]`).
                    let base = {
                        let ty = self.thir.exprs[base.0 as usize].ty;
                        let self_ty = tcx.tys.list(args).first().copied();
                        match (tcx.tys.kind(ty).clone(), self_ty.map(|t| tcx.tys.kind(t).clone())) {
                            (TyKind::Ref(m, inner, r), Some(TyKind::Slice(_))) if matches!(tcx.tys.kind(inner), TyKind::Array(..)) => {
                                let TyKind::Array(elem, _) = *tcx.tys.kind(inner) else { unreachable!() };
                                let target = tcx.tys.intern(TyKind::Ref(m, tcx.tys.intern(TyKind::Slice(elem)), r));
                                let span = self.thir.exprs[base.0 as usize].span;
                                self.push(self.mk_at(e, span, ExprKind::PointerCoercion { cast: PointerCoercion::Unsize, source: base, is_from_as_cast: false }, target))
                            }
                            _ => base,
                        }
                    };
                    let index = self.mirror_expr(index)?;
                    let callee_ty = self.fn_def_ty(def, tcx.tys.list(args));
                    return self.overloaded_place(e, (self.temp_scope(e), self.bid_scope(e)), expr_ty, callee_ty, Box::new([base, index]), span);
                }
                let base = self.mirror_expr(lhs)?;
                let base = self.apply_operand_adjustments_id(e, base)?;
                // builtin indexing is `usize` into an array or slice; typeck records every other
                // index as the overloaded `Index`
                let base_ty = self.thir.exprs[base.0 as usize].ty;
                if !(matches!(tcx.tys.kind(base_ty), TyKind::Array(..) | TyKind::Slice(_)) && self.ety_adjusted(index)? == tcx.tys.usize_) {
                    return self.unresolved(span, "non-builtin index without an Index impl");
                }
                ExprKind::Index { lhs: base, index: self.mirror_expr(index)? }
            }
            A::Unary(ast::UnOp::Deref, x) => {
                let overloaded = tb.adjustments(e.0).and_then(|a| a.operand.first()).and_then(|a| match a.kind {
                    Adjust::Deref(Some((def, args))) => Some((if a.mutbl { self.tcx().mutable_place_op(self.prog(), def).unwrap_or(def) } else { def }, args)),
                    _ => None,
                });
                let arg = self.mirror_expr(x)?;
                let overloaded = overloaded.filter(|_| !self.is_box_ty(self.thir.exprs[arg.0 as usize].ty));
                match overloaded {
                    Some((def, args)) => {
                        let arg = self.autoref(e, arg, self.is_mut_method(def));
                        let callee_ty = self.fn_def_ty(def, tcx.tys.list(args));
                        return self.overloaded_place(e, (self.temp_scope(e), self.bid_scope(e)), expr_ty, callee_ty, Box::new([arg]), span);
                    }
                    None => ExprKind::Deref { arg },
                }
            }
            A::Unary(op @ (ast::UnOp::Not | ast::UnOp::Neg), x) => {
                if let Some(m) = tb.binops.get(&e.0).copied() {
                    let arg = self.mirror_expr(x)?;
                    return self.overloaded_operator(e, m, Box::new([arg]), expr_ty);
                }
                let xe = super::strip_paren(ast, x);
                match (op, ast.expr(xe).kind) {
                    (ast::UnOp::Neg, A::Lit(_)) => ExprKind::Literal { expr: xe, neg: true },
                    (ast::UnOp::Not, _) => ExprKind::Unary { op: MUnOp::Not, arg: self.mirror_expr(x)? },
                    _ => ExprKind::Unary { op: MUnOp::Neg, arg: self.mirror_expr(x)? },
                }
            }
            A::Struct(row) => return self.struct_expr(e, *ast.row(row), expr_ty),
            A::Closure(row) => return self.closure_expr(e, *ast.row(row), expr_ty),
            A::Path(_) => return self.convert_path_expr(e, expr_ty),
            A::ConstBlock(_) => {
                let Some(&def) = self.prog().node_id_to_def_id.get(&(tb.file, crate::def_collector::NodeId::InlineConst(e))) else {
                    return self.unresolved(span, "inline const definition");
                };
                let root_args = self.tcx().generic_indices_of(self.prog(), tb.def).identity_types_consts(&tcx.tys);
                let args = self.full_args(tb.def, &root_args, span)?;
                ExprKind::ConstBlock { def, args }
            }
            A::Repeat(value, _) => {
                let TyKind::Array(_, count) = *tcx.tys.kind(expr_ty) else { return self.unresolved(span, "repeat expression type") };
                ExprKind::Repeat { value: self.mirror_expr(value)?, count }
            }
            A::Return(v) => ExprKind::Return { value: self.opt_expr(v)? },
            A::Break(label, value) => {
                let scope = self.break_target(label.get(), false, span)?;
                ExprKind::Break { label: scope, value: self.opt_expr(value)? }
            }
            A::Continue(label) => {
                let scope = self.break_target(label.get(), true, span)?;
                ExprKind::Continue { label: scope }
            }
            A::Let(pat, init) => {
                let expr = self.mirror_expr(init)?;
                self.pat_hint = span.lo;
                ExprKind::Let { expr, pat: Box::new(self.lower_pattern(pat)?) }
            }
            A::If(cond, then, else_opt) => {
                let if_then_scope = match self.scope_of(Node::Block(then.0), ScopeData::IfThenRescope).or_else(|| self.scope_of(Node::Block(then.0), ScopeData::IfThen)) {
                    Some(scope) => scope,
                    None => return self.unresolved(span, "if-then scope"),
                };
                let cond = self.mirror_expr(cond)?;
                let then = self.block_expr(e, then, expr_ty)?;
                let else_opt = self.opt_expr(else_opt)?;
                ExprKind::If { if_then_scope, cond, then, else_opt }
            }
            A::Match(scrutinee, arms) => {
                let scrutinee = self.mirror_expr(scrutinee)?;
                let mut out = Vec::new();
                for (i, arm) in ast.list(arms).iter().enumerate() {
                    out.push(self.convert_arm(arms.lo + i as u32, arm)?);
                }
                ExprKind::Match { scrutinee, arms: out.into_boxed_slice(), match_source: MatchSource::Normal }
            }
            A::Loop(body, label) => {
                let scope = self.node_scope(Node::Expr(e.0), span)?;
                self.breakables.push(super::Breakable { label: label.get(), scope, is_block: false });
                let body = self.block_expr(e, body, self.unit_ty());
                self.breakables.pop();
                ExprKind::Loop { body: body? }
            }
            A::While(row) => return self.while_expr(e, *ast.row(row), expr_ty),
            A::For(row) => return self.for_expr(e, *ast.row(row), expr_ty),
            A::Try(x) => return self.try_expr(e, x, expr_ty),
            A::Mac(m) => return self.mac_expr(e, *ast.row(m), expr_ty),
            A::Field(base, _) | A::TupleField(base, _) => {
                let Some(&(_, index)) = tb.field_idx.get(&e.0) else { return self.unresolved(span, "field index") };
                let lhs = self.mirror_expr(base)?;
                let lhs = self.apply_operand_adjustments_id(e, lhs)?;
                ExprKind::Field { lhs, variant_index: VariantIdx(0), name: FieldIdx(index) }
            }
            A::Cast(source, cast_ty) => return self.cast_expr(e, source, cast_ty, expr_ty),
            A::Tuple(fields) => ExprKind::Tuple { fields: self.mirror_exprs(ast.list(fields))? },
            A::Array(fields) => ExprKind::Array { fields: self.mirror_exprs(ast.list(fields))? },
            A::Range(a, b, inclusive) => return self.range_expr(e, a, b, inclusive, expr_ty),
            A::Async(..) => return self.closure_aggregate(e, expr_ty, Some(crate::mir::Movability::Static)),
            A::Await(x) => return self.await_expr(e, x, expr_ty),
            A::Yield(_) => return self.not_built(span, "yield"),
            A::Box(_) => return self.not_built(span, "box expression"),
            A::Underscore => return self.unresolved(span, "`_` expression"),
        };
        Ok(self.mk(e, kind, expr_ty))
    }

    fn opt_expr(&mut self, e: OptExprId) -> BuildResult<Option<ExprId>> {
        match e.get() {
            Some(x) => Ok(Some(self.mirror_expr(x)?)),
            None => Ok(None),
        }
    }

    /// A block of `e` as an expression of type `ty` (the `then` of an `if`, a loop body).
    pub(super) fn block_expr(&mut self, e: ast::ExprId, b: ast::BlockId, ty: TyId) -> BuildResult<ExprId> {
        let block = self.mirror_block(b)?;
        let blk = self.ast().block(b);
        let span = self.span(blk.lo, blk.hi);
        Ok(self.push(self.mk_at(e, span, ExprKind::Block { block }, ty)))
    }

    fn apply_operand_adjustments_id(&mut self, node: ast::ExprId, id: ExprId) -> BuildResult<ExprId> {
        if self.tb().adjustments(node.0).map_or(true, |a| a.operand.is_empty()) {
            return Ok(id);
        }
        let expr = self.thir.exprs[id.0 as usize].clone();
        let adjusted = self.apply_operand_adjustments(node, expr)?;
        Ok(self.push(adjusted))
    }

    /// The `&mut` autoref of an overloaded assignment operator's left side (a two-phase borrow).
    fn autoref_two_phase(&mut self, e: ast::ExprId, lhs: ExprId) -> ExprId {
        let operand_ty = self.thir.exprs[lhs.0 as usize].ty;
        let ty = self.ref_ty(true, operand_ty);
        let span = self.thir.exprs[lhs.0 as usize].span;
        self.push(self.mk_at(e, span, ExprKind::Borrow { borrow_kind: mut_borrow(true), arg: lhs }, ty))
    }

    /// The scope a `break` / `continue` targets.
    fn break_target(&self, label: Option<ast::Ident>, is_continue: bool, span: Span) -> BuildResult<crate::region_scope::ScopeId> {
        let file = self.file();
        for target in self.breakables.iter().rev() {
            match label {
                None => {
                    if !target.is_block {
                        return Ok(target.scope);
                    }
                }
                Some(l) => {
                    if target.label.is_some_and(|t| file.text(t) == file.text(l)) && !(is_continue && target.is_block) {
                        return Ok(target.scope);
                    }
                }
            }
        }
        self.unresolved(span, "break target")
    }

    fn convert_arm(&mut self, node: u32, arm: &ast::Arm) -> BuildResult<ArmId> {
        let ast = self.ast();
        let pat_span = self.pat_span(arm.pat);
        let body_hi = ast.expr(arm.body).hi;
        let span = self.span(pat_span.lo, body_hi.max(pat_span.hi));
        let scope = self.node_scope(Node::Arm(node), span)?;
        self.pat_hint = pat_span.lo;
        let pattern = Box::new(self.lower_pattern(arm.pat)?);
        let guard = match arm.guard {
            Some(g) => Some(self.mirror_expr(g)?),
            None => None,
        };
        let body = self.mirror_expr(arm.body)?;
        Ok(self.thir.alloc_arm(Arm { pattern, guard, body, node, scope, span }))
    }

    // ---------------------------------------------------------------- paths

    /// The user-written type of a path to a function, constructor or constant.
    fn path_user_ty(&self, e: ast::ExprId) -> UserTy {
        self.user_ty_of(UserTypeKey::of_node(e.0))
    }

    /// thir/util.rs:`user_args_applied_to_ty_of_hir_id` for an expression: the annotation of a unit
    /// struct / variant names the ADT itself.
    fn adt_user_ty(&self, e: ast::ExprId, adt: DefId) -> UserTy {
        let mut user_ty = self.user_ty_of(UserTypeKey::of_node(e.0))?;
        if let UserTypeKind::TypeOf(did, _) = &mut user_ty.value.kind {
            *did = adt;
        }
        Some(user_ty)
    }

    fn convert_path_expr(&mut self, e: ast::ExprId, expr_ty: TyId) -> BuildResult<Expr> {
        let (tb, tcx, prog) = (self.tb(), self.tcx(), self.prog());
        let span = self.espan(e);
        let res = tb.res.get(&e.0).copied();
        let def = match res {
            Some(Res::Local(index)) => {
                let var = LocalVarId(index);
                let kind = if self.is_upvar(index) { ExprKind::UpvarRef { var } } else { ExprKind::VarRef { id: var } };
                return Ok(self.mk(e, kind, expr_ty));
            }
            Some(Res::ConstParam(index)) => {
                let param = crate::consts::ParamConst { index };
                return Ok(self.mk(e, ExprKind::ConstParam { param, def: crate::program::NO_DEF }, expr_ty));
            }
            Some(Res::Def(d)) => d,
            _ => return Err(self.error(span, UnsupportedReason::TypeckTainted)),
        };
        let compact: Vec<TyId> = tb.res_args.get(&e.0).map(|l| tcx.tys.list(*l).to_vec()).unwrap_or_default();
        let kind = match prog.def(def).kind {
            // A regular function or a constructor function.
            DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => ExprKind::ZstLiteral { user_ty: self.path_user_ty(e) },
            DefKind::Variant | DefKind::Struct => {
                let tuple_or_fn = matches!(tcx.tys.kind(expr_ty), TyKind::FnDef(..));
                if tuple_or_fn {
                    ExprKind::ZstLiteral { user_ty: self.path_user_ty(e) }
                } else {
                    // A unit struct/variant which is used as a value.
                    let (adt, args) = self.adt_args(expr_ty, span)?;
                    let variant_index = if prog.def(def).kind == DefKind::Variant { VariantIdx(prog.def(def).sub) } else { VariantIdx(0) };
                    ExprKind::Adt(Box::new(AdtExpr { adt_def: adt, variant_index, args, user_ty: self.adt_user_ty(e, adt), fields: Box::new([]), base: AdtExprBase::None }))
                }
            }
            DefKind::Const | DefKind::AssocConst => {
                let args = self.full_args(def, &compact, span)?;
                ExprKind::NamedConst { def, args, user_ty: self.path_user_ty(e) }
            }
            // A source Rust `path::to::STATIC` is a place expr like `*&ident` is. In THIR, we make them
            // exactly equivalent by inserting the implied `*&` or `*&raw`, but distinguish between
            // `&STATIC` and `&THREAD_LOCAL` as they have different semantics.
            DefKind::Static | DefKind::ForeignStatic => {
                let mutable = match prog.def(def).kind {
                    DefKind::ForeignStatic => true,
                    _ => {
                        let d = prog.def(def);
                        matches!(prog.files[d.file as usize].ast.item(d.item).kind, ast::ItemKind::Static(true, _, _))
                    }
                };
                let ptr_ty = if mutable { tcx.tys.intern(TyKind::Ptr(true, expr_ty)) } else { self.ref_ty(false, expr_ty) };
                let inner = if crate::mir::transform::promote_consts::is_thread_local_static(prog, def) {
                    ExprKind::ThreadLocalRef(def)
                } else {
                    ExprKind::StaticRef { def, ty: ptr_ty }
                };
                let arg = self.push(self.mk(e, inner, ptr_ty));
                ExprKind::Deref { arg }
            }
            _ => return self.unresolved(span, "path resolution"),
        };
        Ok(self.mk(e, kind, expr_ty))
    }

    // ---------------------------------------------------------------- calls

    fn method_call(&mut self, e: ast::ExprId, recv: ast::ExprId, row: ast::MethodCallRow, expr_ty: TyId) -> BuildResult<Expr> {
        let (tb, tcx, prog) = (self.tb(), self.tcx(), self.prog());
        let span = self.espan(e);
        let Some(m) = tb.methods.get(&e.0).copied() else { return self.unresolved(span, "method resolution") };
        let args_list = tcx.tys.list(m.args);
        let callee_ty = self.fn_def_ty(m.def, args_list);
        let fn_span = self.span(row.name.lo, span.hi);
        // Rewrite `a.b(c)` into UFCS form like `Trait::b(a, c)`.
        let fun = self.push(self.mk_at(e, fn_span, ExprKind::ZstLiteral { user_ty: self.user_ty_of(UserTypeKey::Expr(e.0)) }, callee_ty));
        let receiver = self.method_receiver(e, recv, &m)?;
        let mut args = vec![receiver];
        for &a in ast::Ast::list(self.ast(), row.args) {
            args.push(self.mirror_arg(a)?);
        }
        let _ = prog;
        Ok(self.mk(e, ExprKind::Call { ty: callee_ty, fun, args: args.into_boxed_slice(), from_hir_call: true, fn_span }, expr_ty))
    }

    /// The receiver with its autoderef, autoref (two-phase when mutable) and unsizing made explicit.
    fn method_receiver(&mut self, e: ast::ExprId, recv: ast::ExprId, m: &crate::typeck::MethodRes) -> BuildResult<ExprId> {
        let (tcx, prog) = (self.tcx(), self.prog());
        let r = self.mirror_expr(recv)?;
        let r = self.apply_operand_adjustments_id(e, r)?;
        let cur = self.thir.exprs[r.0 as usize].clone();
        let span = cur.span;
        if m.autoref != 0 {
            let mutable = m.autoref == 2;
            let ty = self.ref_ty(mutable, cur.ty);
            let borrow_kind = if mutable { mut_borrow(true) } else { BorrowKind::Shared };
            let borrow = self.push(self.mk_at(e, span, ExprKind::Borrow { borrow_kind, arg: r }, ty));
            // an array receiver of a slice method: the borrow unsizes
            if let TyKind::Array(elem, _) = *tcx.tys.kind(cur.ty) {
                if let Some(sig) = tcx.fn_sig(prog, m.def) {
                    if let Some(&p0) = sig.params.first() {
                        let p0 = tcx.tys.subst(p0, tcx.tys.list(m.args));
                        let p0 = tcx.try_normalize_erasing_regions(prog, self.root.cx.env, p0).unwrap_or(p0);
                        if matches!(*tcx.tys.kind(p0), TyKind::Ref(_, inner, _) if matches!(tcx.tys.kind(inner), TyKind::Slice(_))) {
                            let slice = tcx.tys.intern(TyKind::Slice(elem));
                            let target = self.ref_ty(mutable, slice);
                            return Ok(self.push(self.mk_at(e, span, ExprKind::PointerCoercion { cast: PointerCoercion::Unsize, source: borrow, is_from_as_cast: false }, target)));
                        }
                    }
                }
            }
            return Ok(borrow);
        }
        // A reference receiver taken by value is reborrowed (`&*x` / `&mut *x`), like rustc's probe does.
        if let TyKind::Ref(mutable, pointee, _) = *tcx.tys.kind(cur.ty) {
            let place = self.push(self.mk_at(e, span, ExprKind::Deref { arg: r }, pointee));
            let borrow_kind = if mutable { mut_borrow(true) } else { BorrowKind::Shared };
            return Ok(self.push(self.mk_at(e, span, ExprKind::Borrow { borrow_kind, arg: place }, cur.ty)));
        }
        Ok(r)
    }

    fn call(&mut self, e: ast::ExprId, fun: ast::ExprId, args: &[ast::ExprId], expr_ty: TyId) -> BuildResult<Expr> {
        let (tb, tcx, prog, ast) = (self.tb(), self.tcx(), self.prog(), self.ast());
        let span = self.espan(e);
        let kept: Vec<ast::ExprId>;
        let args: &[ast::ExprId] = match tb.legacy_args.get(&e.0) {
            Some(skip) => {
                kept = args.iter().enumerate().filter(|(i, _)| !skip.contains(&(*i as u32))).map(|(_, a)| *a).collect();
                &kept
            }
            None => args,
        };
        // Tuple-like ADTs are represented as `ExprKind::Call`; we convert them here.
        let callee_expr = super::strip_paren(ast, fun);
        if let (A::Path(_), Some(Res::Def(d))) = (ast.expr(callee_expr).kind, tb.res.get(&callee_expr.0).copied()) {
            if matches!(prog.def(d).kind, DefKind::Variant | DefKind::Struct) && matches!(tcx.tys.kind(expr_ty), TyKind::Adt(..)) {
                let (adt, adt_args) = self.adt_args(expr_ty, span)?;
                let variant_index = if prog.def(d).kind == DefKind::Variant { VariantIdx(prog.def(d).sub) } else { VariantIdx(0) };
                let mut fields = Vec::new();
                for (i, &a) in args.iter().enumerate() {
                    fields.push(FieldExpr { name: FieldIdx(i as u32), expr: self.mirror_arg(a)? });
                }
                let user_ty = self.adt_user_ty(callee_expr, adt);
                return Ok(self.mk(e, ExprKind::Adt(Box::new(AdtExpr { adt_def: adt, variant_index, args: adt_args, user_ty, fields: fields.into_boxed_slice(), base: AdtExprBase::None })), expr_ty));
            }
        }
        // The callee with its autoderef.
        let f = self.mirror_expr(fun)?;
        let f = self.apply_operand_adjustments_id(e, f)?;
        let callee = self.thir.exprs[f.0 as usize].clone();
        let selection = tb.call_traits.get(&e.0).copied();
        let builtin = matches!(tcx.tys.kind(callee.ty), TyKind::FnDef(..) | TyKind::FnPtr(..));
        if builtin && selection.is_none() {
            let mut mirrored = Vec::new();
            for &a in args {
                mirrored.push(self.mirror_arg(a)?);
            }
            return Ok(self.mk(e, ExprKind::Call { ty: callee.ty, fun: f, args: mirrored.into_boxed_slice(), from_hir_call: true, fn_span: span }, expr_ty));
        }
        // The callee is something implementing `Fn`, `FnMut` or `FnOnce`: rewrite `f(u, v)` into
        // `FnOnce::call_once(f, (u, v))` (or the `call` / `call_mut` of the selected trait).
        let cx = self.root.cx.eval;
        let Some(kind) = tb.call_kind(tcx, cx, e, callee.ty) else { return self.unresolved(span, "call trait selection") };
        let trait_def = match selection {
            Some(s) => s.trait_def,
            None => {
                let item = match kind {
                    crate::upvar::ClosureKind::Fn => LangItem::Fn,
                    crate::upvar::ClosureKind::FnMut => LangItem::FnMut,
                    crate::upvar::ClosureKind::FnOnce => LangItem::FnOnce,
                };
                match tcx.lang_items.get(item) {
                    Some(d) => d,
                    None => return self.unresolved(span, "Fn trait lang item"),
                }
            }
        };
        if matches!(tcx.lang_items.by_def.get(&trait_def), Some(LangItem::AsyncFn | LangItem::AsyncFnMut | LangItem::AsyncFnOnce)) {
            return self.not_built(span, "async call trait");
        }
        let Some(&method) = prog.traits[prog.def(trait_def).sub as usize].iter().find(|&&d| prog.def(d).kind == DefKind::AssocFn) else {
            return self.unresolved(span, "call trait method");
        };
        let callee_id = match kind {
            crate::upvar::ClosureKind::Fn => self.autoref(e, f, false),
            crate::upvar::ClosureKind::FnMut => self.autoref(e, f, true),
            crate::upvar::ClosureKind::FnOnce => f,
        };
        let mut mirrored = Vec::new();
        let mut arg_tys = Vec::new();
        for &a in args {
            arg_tys.push(self.ety_adjusted(a)?);
            mirrored.push(self.mirror_arg(a)?);
        }
        let tupled = match selection {
            Some(s) => s.tupled_args,
            None => tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&arg_tys))),
        };
        let self_ty = selection.map_or(callee.ty, |s| s.self_ty);
        let method_ty = self.fn_def_ty(method, &[self_ty, tupled]);
        let fun_id = self.push(self.mk_at(e, callee.span, ExprKind::ZstLiteral { user_ty: None }, method_ty));
        let tuple = self.push(self.mk(e, ExprKind::Tuple { fields: mirrored.into_boxed_slice() }, tupled));
        Ok(self.mk(e, ExprKind::Call { ty: method_ty, fun: fun_id, args: Box::new([callee_id, tuple]), from_hir_call: true, fn_span: span }, expr_ty))
    }

    // ---------------------------------------------------------------- aggregates

    fn struct_expr(&mut self, e: ast::ExprId, row: ast::StructRow, expr_ty: TyId) -> BuildResult<Expr> {
        let (tb, tcx, prog, ast) = (self.tb(), self.tcx(), self.prog(), self.ast());
        let span = self.espan(e);
        let TyKind::Adt(adt_def, _, _) = *tcx.tys.kind(expr_ty) else { return self.unresolved(span, "struct literal type") };
        let (adt, args) = self.adt_args(expr_ty, span)?;
        let variant_index = match tb.res.get(&e.0).copied() {
            Some(Res::Def(d)) if prog.def(d).kind == DefKind::Variant => VariantIdx(prog.def(d).sub),
            _ => VariantIdx(0),
        };
        let mut fields = Vec::new();
        for f in ast.list(row.fields) {
            let Some(idx) = self.field_index(adt_def, variant_index.0, f.name) else { return self.unresolved(span, "unknown field") };
            fields.push(FieldExpr { name: FieldIdx(idx), expr: self.mirror_expr(f.expr)? });
        }
        // `S { a, .. }`: each omitted field takes the const holding its declared default
        // (rustc builds `AdtExprBase::DefaultFields`, whose operands are exactly these consts)
        if row.rest {
            let field_args = tcx.tys.list(match *tcx.tys.kind(expr_ty) {
                TyKind::Adt(_, a, _) => a,
                _ => unreachable!(),
            });
            for &(index, def) in tb.struct_defaults.get(&e.0).map(|v| v.as_slice()).unwrap_or(&[]) {
                if fields.iter().any(|f: &FieldExpr| f.name.0 == index) {
                    continue;
                }
                let ty = tcx.field_ty(prog, self.root.cx.env, adt_def, variant_index.0, index, field_args);
                let args = self.full_args(def, &[], span)?;
                let expr = self.push(self.mk(e, ExprKind::NamedConst { def, args, user_ty: None }, ty));
                fields.push(FieldExpr { name: FieldIdx(index), expr });
            }
        }
        let base = match row.base.get() {
            Some(base) => {
                let n = tcx.adts.get(&adt_def).and_then(|a| a.variants.get(variant_index.0 as usize)).map_or(0, |v| v.fields.len());
                let (_, gargs) = (adt, &args);
                let _ = gargs;
                let tys = tcx.tys.list(match *tcx.tys.kind(expr_ty) {
                    TyKind::Adt(_, a, _) => a,
                    _ => unreachable!(),
                });
                let field_types: Vec<TyId> = (0..n).map(|i| tcx.field_ty(prog, self.root.cx.env, adt_def, variant_index.0, i as u32, tys)).collect();
                AdtExprBase::Base(FruInfo { base: self.mirror_expr(base)?, field_types: field_types.into_boxed_slice() })
            }
            None => AdtExprBase::None,
        };
        let user_ty = self.user_ty_of(UserTypeKey::Expr(e.0));
        Ok(self.mk(e, ExprKind::Adt(Box::new(AdtExpr { adt_def: adt, variant_index, args, user_ty, fields: fields.into_boxed_slice(), base })), expr_ty))
    }

    fn range_expr(&mut self, e: ast::ExprId, a: OptExprId, b: OptExprId, inclusive: bool, expr_ty: TyId) -> BuildResult<Expr> {
        let (tcx, prog) = (self.tcx(), self.prog());
        let span = self.espan(e);
        let (start, end) = (self.opt_expr(a)?, self.opt_expr(b)?);
        if inclusive {
            if let (Some(start), Some(end)) = (start, end) {
                // `RangeInclusive::new(start, end)`
                let Some(new) = tcx.lang_items.get(LangItem::RangeInclusiveNew) else { return self.unresolved(span, "RangeInclusive::new") };
                let TyKind::Adt(_, adt_args, _) = *tcx.tys.kind(expr_ty) else { return self.unresolved(span, "range type") };
                let callee_ty = self.fn_def_ty(new, tcx.tys.list(adt_args));
                let fun = self.push(self.mk(e, ExprKind::ZstLiteral { user_ty: None }, callee_ty));
                return Ok(self.mk(e, ExprKind::Call { ty: callee_ty, fun, args: Box::new([start, end]), from_hir_call: false, fn_span: span }, expr_ty));
            }
        }
        let (adt, args) = self.adt_args(expr_ty, span)?;
        let mut fields = Vec::new();
        let mut idx = 0u32;
        for x in [start, end].into_iter().flatten() {
            fields.push(FieldExpr { name: FieldIdx(idx), expr: x });
            idx += 1;
        }
        let _ = prog;
        Ok(self.mk(e, ExprKind::Adt(Box::new(AdtExpr { adt_def: adt, variant_index: VariantIdx(0), args, user_ty: None, fields: fields.into_boxed_slice(), base: AdtExprBase::None })), expr_ty))
    }

    /// thir/cx/expr.rs `Closure`: the captured places as expressions in the creating body.
    fn closure_expr(&mut self, e: ast::ExprId, row: ast::ClosureRow, expr_ty: TyId) -> BuildResult<Expr> {
        let mut aggregate = self.closure_aggregate(e, expr_ty, None)?;
        if row.is_async {
            // `async |..| ..`: a coroutine closure whose body creates the coroutine
            if let ExprKind::Closure(c) = &mut aggregate.kind {
                c.kind = super::ClosureExprKind::CoroutineClosure;
            }
        }
        Ok(aggregate)
    }

    /// The aggregate of a closure or coroutine: the captured places (by value or borrowed).
    fn closure_aggregate(&mut self, e: ast::ExprId, expr_ty: TyId, movability: Option<crate::mir::Movability>) -> BuildResult<Expr> {
        let (temp, span) = (self.temp_scope(e), self.espan(e));
        self.closure_aggregate_at(e.0, temp, span, expr_ty, movability)
    }

    /// The aggregate of the closure / coroutine `ce` created at a site with temporary scope `temp`
    /// (an async fn creates its body coroutine at no expression).
    pub(super) fn closure_aggregate_at(&mut self, ce: u32, temp: Option<crate::region_scope::ScopeId>, span: Span, expr_ty: TyId, movability: Option<crate::mir::Movability>) -> BuildResult<Expr> {
        let (tb, tcx) = (self.tb(), self.tcx());
        let (closure, args, kind, tupled) = self.closure_parts(expr_ty, span)?;
        let Some(caps) = tb.min_captures.get(&ce) else { return self.unresolved(span, "closure captures") };
        let upvar_tys: Vec<TyId> = match *tcx.tys.kind(tupled) {
            TyKind::Tuple(l) => tcx.tys.list(l).to_vec(),
            _ => return self.unresolved(span, "closure upvar types"),
        };
        if upvar_tys.len() != caps.captures.len() {
            return self.unresolved(span, "closure upvar count");
        }
        let mut upvars = Vec::new();
        for (capture, &ty) in caps.captures.iter().zip(&upvar_tys) {
            let place = self.captured_place_expr(temp, span, &capture.place);
            let kind = match capture.info.kind {
                crate::upvar::CaptureKind::ByValue => {
                    upvars.push(self.push(place));
                    continue;
                }
                crate::upvar::CaptureKind::ByRef(crate::expr_use::BorrowKind::Imm) => BorrowKind::Shared,
                crate::upvar::CaptureKind::ByRef(crate::expr_use::BorrowKind::UniqueImm) => BorrowKind::Mut { kind: MutBorrowKind::ClosureCapture },
                crate::upvar::CaptureKind::ByRef(crate::expr_use::BorrowKind::Mut) => BorrowKind::Mut { kind: MutBorrowKind::Default },
            };
            let arg = self.push(place);
            upvars.push(self.push(Expr { kind: ExprKind::Borrow { borrow_kind: kind, arg }, ty, temp_scope: temp, bid: None, span }));
        }
        let mut fake_reads = Vec::new();
        for (place, cause, node) in &caps.fake_reads {
            let expr = self.captured_place_expr(temp, span, place);
            let cause = match cause {
                crate::expr_use::FakeReadCause::Let => FakeReadCause::ForLet(None),
                crate::expr_use::FakeReadCause::Matched => FakeReadCause::ForMatchedPlace(None),
            };
            fake_reads.push((self.push(expr), cause, *node));
        }
        Ok(Expr { kind: ExprKind::Closure(Box::new(ClosureExpr { closure, args, upvars: upvars.into_boxed_slice(), movability, fake_reads, kind })), ty: expr_ty, temp_scope: temp, bid: None, span })
    }

    /// The place a closure captures, as an expression of the creating body (starting from the
    /// variable, which may itself be an upvar of this body).
    fn captured_place_expr(&mut self, temp: Option<crate::region_scope::ScopeId>, span: Span, place: &crate::expr_use::Place) -> Expr {
        use crate::expr_use::Projection;
        let var = LocalVarId(place.root);
        let kind = if self.is_upvar(place.root) { ExprKind::UpvarRef { var } } else { ExprKind::VarRef { id: var } };
        let mut expr = Expr { kind, ty: place.root_ty, temp_scope: temp, bid: None, span };
        for (proj, ty) in &place.projections {
            let kind = match *proj {
                Projection::Deref => ExprKind::Deref { arg: self.push(expr) },
                Projection::Field(variant, field) => ExprKind::Field { lhs: self.push(expr), variant_index: VariantIdx(variant), name: FieldIdx(field) },
                // we don't capture these projections, so we can ignore them here
                Projection::Index | Projection::Subslice => continue,
            };
            expr = Expr { kind, ty: *ty, temp_scope: temp, bid: None, span };
        }
        expr
    }

    // ---------------------------------------------------------------- casts

    /// The fn pointer type a fn item / capture-free closure coerces to (typeck's `fn_ptr_of`).
    fn fn_ptr_of(&self, ty: TyId) -> Option<TyId> {
        let (tcx, prog) = (self.tcx(), self.prog());
        match tcx.tys.kind(ty).clone() {
            TyKind::FnPtr(..) => Some(ty),
            TyKind::FnDef(d, args, regions) => {
                let sig = tcx.fn_item_sig(prog, d).cloned()?;
                let poly = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&sig.params), sig.ret, sig.header, sig.bound_regions));
                let args = tcx.complete_args(prog, d, tcx.tys.list(args), regions);
                Some(tcx.tys.erase_regions(tcx.instantiate_ty(prog, d, &args, poly)))
            }
            TyKind::Closure(_, _, closure_args) => Some(closure_args.closure_sig_as_fn_ptr_ty),
            _ => None,
        }
    }

    fn cast_expr(&mut self, e: ast::ExprId, source: ast::ExprId, cast_ty: ast::TyId, expr_ty: TyId) -> BuildResult<Expr> {
        let (tb, tcx, prog, ast) = (self.tb(), self.tcx(), self.prog(), self.ast());
        let span = self.espan(e);
        let adjusted = self.ety_adjusted(source)?;
        let source_ty = self.ety(source)?;
        let saved = self.cast_source.replace(source);
        let mirrored = self.mirror_expr(source);
        self.cast_source = saved;
        let mut mirrored = mirrored?;
        // `f as *mut ()` / `f as usize` on a fn item (or capture-free closure): rustc's cast check
        // first reifies it to a fn pointer
        if matches!(tcx.tys.kind(adjusted), TyKind::FnDef(..) | TyKind::Closure(..)) && !matches!(tcx.tys.kind(expr_ty), TyKind::FnPtr(..) | TyKind::FnDef(..) | TyKind::Closure(..)) {
            if let Some(fn_ptr) = self.fn_ptr_of(adjusted) {
                let TyKind::FnPtr(_, _, header, _) = *tcx.tys.kind(fn_ptr) else { unreachable!() };
                let cast = if matches!(tcx.tys.kind(adjusted), TyKind::Closure(..)) { PointerCoercion::ClosureFnPointer(header.safety) } else { PointerCoercion::ReifyFnPointer(header.safety) };
                mirrored = self.push(self.mk(e, ExprKind::PointerCoercion { cast, source: mirrored, is_from_as_cast: true }, fn_ptr));
            }
        }
        // rustc cast.rs: a raw-pointer to raw-pointer cast is never turned into a coercion, because the
        // pointee types may differ only in regions, which HIR typeck cannot see (borrowck must check them)
        let ptr_to_ptr = matches!(tcx.tys.kind(source_ty), TyKind::Ptr(..)) && matches!(tcx.tys.kind(expr_ty), TyKind::Ptr(..));
        let kind = if !ptr_to_ptr && (adjusted == expr_ty || tcx.tys.erase_regions(adjusted) == tcx.tys.erase_regions(expr_ty)) {
            // a coercion cast (or a no-op): the value converted by the coercion
            ExprKind::Use { source: mirrored }
        } else if matches!(tcx.tys.kind(source_ty), TyKind::Ref(..)) {
            // `&[T; N] as *const T`: the element type of the source matches the pointee of the destination
            ExprKind::PointerCoercion { source: mirrored, cast: PointerCoercion::ArrayToPointer, is_from_as_cast: true }
        } else {
            // casting an enum variant's discriminant
            let src = super::strip_paren(ast, source);
            let discr = match (ast.expr(src).kind, tb.res.get(&src.0).copied(), tcx.tys.kind(source_ty)) {
                (A::Path(_), Some(Res::Def(d)), TyKind::Adt(adt, _, _)) if prog.def(d).kind == DefKind::Variant => {
                    let idx = prog.def(d).sub;
                    tcx.adts.get(adt).and_then(|a| a.discriminant_for_variant(tcx, prog, idx))
                }
                _ => None,
            };
            match discr {
                Some(discr) => {
                    let (bytes, _) = crate::types::Types::int_info(discr.ty);
                    let (lit, _) = ScalarInt::truncate_from_uint(discr.val, bytes as u8);
                    let discr_ty = tcx.tys.prim(discr.ty);
                    let literal = self.push(self.mk(e, ExprKind::NonHirLiteral { lit, user_ty: None }, discr_ty));
                    ExprKind::Cast { source: literal }
                }
                None => ExprKind::Cast { source: mirrored },
            }
        };
        let cast = self.mk(e, kind, expr_ty);
        // Check for a user-given type annotation on this `cast`.
        match self.user_ty_of(UserTypeKey::Type(cast_ty.0)) {
            Some(user_ty) => {
                let source = self.push(cast);
                Ok(self.mk_at(e, span, ExprKind::ValueTypeAscription { source, user_ty: Some(user_ty), user_ty_span: span }, expr_ty))
            }
            None => Ok(cast),
        }
    }
}
