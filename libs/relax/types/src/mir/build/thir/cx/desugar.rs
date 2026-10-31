//! The desugarings rustc's AST lowering does before THIR exists: `while`, `for`, `?` and Relax's
//! builtin macros (`matches!`, `assert!`, the print and panic families). Each uses the scopes the
//! region tree (`region_scope.rs`) enters for the same construct.
use super::{Breakable, Cx};
use crate::ast::{self, ExprKind as A};
use crate::consts::ScalarInt;
use crate::lang_items::LangItem;
use crate::mir::build::thir::{Arm, Block, BlockSafety, Expr, ExprId, ExprKind, FieldPat, LocalVarId, MatchSource, Pat, PatKind, Stmt, StmtKind};
use crate::mir::build::BuildResult;
use crate::mir::{BorrowKind, FieldIdx, MutBorrowKind, Span, VariantIdx};
use crate::program::{DefId, DefKind};
use crate::region_scope::{Node, ScopeData, ScopeId, Synthetic};
use crate::typeck::Res;
use crate::types::{TyId, TyKind};

impl Cx<'_> {
    fn synthetic_scope(&self, e: ast::ExprId, which: Synthetic, data: ScopeData, span: Span) -> BuildResult<ScopeId> {
        match self.scope_of(Node::Synthetic(e.0, which), data) {
            Some(scope) => Ok(scope),
            None => self.unresolved(span, "scope of a desugaring"),
        }
    }

    /// `value` wrapped in the scope of its HIR node (every expression of rustc's HIR is).
    fn scope_wrap(&mut self, e: ast::ExprId, scope: ScopeId, value: ExprId) -> ExprId {
        let inner = &self.thir.exprs[value.0 as usize];
        let wrapped = Expr { kind: ExprKind::Scope { region_scope: scope, node: e.0, value }, ty: inner.ty, temp_scope: inner.temp_scope, bid: inner.bid, span: inner.span };
        self.push(wrapped)
    }

    fn lit_bool(&self, e: ast::ExprId, value: bool, span: Span) -> Expr {
        let lit = ScalarInt::try_from_uint(value as u128, 1).expect("one byte");
        self.mk_at(e, span, ExprKind::NonHirLiteral { lit, user_ty: None }, self.bool_ty())
    }

    /// `ty` with its projections normalized (the result type of a trait method).
    fn normalize(&self, ty: TyId) -> TyId {
        self.tcx().try_normalize_erasing_regions(self.prog(), self.root.cx.env, ty).unwrap_or(ty)
    }

    /// The return type of the trait method `def` at `args`.
    fn method_output(&self, def: DefId, args: &[TyId], span: Span) -> BuildResult<TyId> {
        let tcx = self.tcx();
        let Some(sig) = tcx.fn_sig(self.prog(), def) else { return self.unresolved(span, "method signature") };
        Ok(tcx.tys.erase_regions(self.normalize(tcx.tys.subst(sig.ret, args))))
    }

    /// The index of an enum's variant that is the lang item `item`.
    fn lang_variant(&self, adt: DefId, item: LangItem, span: Span) -> BuildResult<VariantIdx> {
        let (tcx, prog) = (self.tcx(), self.prog());
        let Some(variant) = tcx.lang_items.get(item) else { return self.unresolved(span, "variant lang item") };
        let _ = adt;
        Ok(VariantIdx(prog.def(variant).sub))
    }

    fn variant_pat(&self, ty: TyId, variant: VariantIdx, subpatterns: Vec<FieldPat>, span: Span) -> BuildResult<Pat> {
        let (adt, args) = self.adt_args(ty, span)?;
        Ok(Pat { ty, span, extra: None, kind: PatKind::Variant { adt_def: adt, args, variant_index: variant, subpatterns } })
    }

    // ---------------------------------------------------------------- while

    /// `while cond { body }` is `loop { if cond { body } else { break } }`.
    pub(super) fn while_expr(&mut self, e: ast::ExprId, row: ast::WhileRow, expr_ty: TyId) -> BuildResult<Expr> {
        let span = self.espan(e);
        let loop_scope = self.node_scope(Node::Expr(e.0), span)?;
        let iteration = self.synthetic_scope(e, Synthetic::Iteration, ScopeData::Node, span)?;
        let iteration_blocks = self.synthetic_scope(e, Synthetic::Iteration, ScopeData::Destruction, span)?;
        let unit = self.unit_ty();
        self.breakables.push(Breakable { label: row.label, scope: loop_scope, is_block: false });
        let built = (|| -> BuildResult<ExprId> {
            let if_then_scope = match self.scope_of(Node::Block(row.body.0), ScopeData::IfThenRescope).or_else(|| self.scope_of(Node::Block(row.body.0), ScopeData::IfThen)) {
                Some(scope) => scope,
                None => return self.unresolved(span, "while-body scope"),
            };
            let cond = self.mirror_expr(row.cond)?;
            let then = self.block_expr(e, row.body, unit)?;
            // `else { break; }` as rustc's THIR has it: the `!` block and the statement's `break` are
            // each coerced to `()`
            let never = self.tcx().tys.never;
            let else_scope = self.synthetic_scope(e, Synthetic::WhileElse, ScopeData::Node, span)?;
            let else_block_scope = self.synthetic_scope(e, Synthetic::WhileElseBlock, ScopeData::Node, span)?;
            let break_stmt_scope = self.synthetic_scope(e, Synthetic::WhileBreak, ScopeData::Node, span)?;
            let break_scope = self.synthetic_scope(e, Synthetic::WhileBreakExpr, ScopeData::Node, span)?;
            let brk = self.push(Expr { kind: ExprKind::Break { label: loop_scope, value: None }, ty: never, temp_scope: Some(break_scope), bid: None, span });
            let brk = self.push(Expr { kind: ExprKind::NeverToAny { source: brk }, ty: unit, temp_scope: Some(break_scope), bid: None, span });
            let brk = self.scope_wrap(e, break_scope, brk);
            let stmt = self.thir.alloc_stmt(Stmt { kind: StmtKind::Expr { scope: break_stmt_scope, expr: brk } });
            let else_block = self.thir.alloc_block(Block { targeted_by_break: false, region_scope: else_block_scope, span, stmts: Box::new([stmt]), expr: None, safety_mode: BlockSafety::Safe });
            let else_expr = self.push(Expr { kind: ExprKind::Block { block: else_block }, ty: never, temp_scope: Some(else_scope), bid: None, span });
            let else_expr = self.push(Expr { kind: ExprKind::NeverToAny { source: else_expr }, ty: unit, temp_scope: Some(else_scope), bid: None, span });
            let else_opt = Some(self.scope_wrap(e, else_scope, else_expr));
            let if_expr = self.mk(e, ExprKind::If { if_then_scope, cond, then, else_opt }, unit);
            let if_id = self.push(if_expr);
            let scoped = self.push(self.mk(e, ExprKind::Scope { region_scope: iteration, node: e.0, value: if_id }, unit));
            let block = self.thir.alloc_block(Block { targeted_by_break: false, region_scope: iteration_blocks, span, stmts: Box::new([]), expr: Some(scoped), safety_mode: BlockSafety::Safe });
            Ok(self.push(self.mk(e, ExprKind::Block { block }, unit)))
        })();
        self.breakables.pop();
        Ok(self.mk(e, ExprKind::Loop { body: built? }, expr_ty))
    }

    // ---------------------------------------------------------------- for

    /// `for pat in head { body }` (rustc_ast_lowering lower_expr_for):
    /// `DropTemps(match IntoIterator::into_iter(head) { mut iter => loop { match Iterator::next(&mut iter) { None => break, Some(pat) => body } } })`
    pub(super) fn for_expr(&mut self, e: ast::ExprId, row: ast::ForRow, expr_ty: TyId) -> BuildResult<Expr> {
        let (tb, tcx) = (self.tb(), self.tcx());
        let span = self.espan(e);
        let unit = self.unit_ty();
        let never = tcx.tys.never;
        let (Some(&(into_def, into_args, iter_ty)), Some(&(next_def, next_args))) = (tb.for_into.get(&row.iter.0), tb.for_next.get(&row.iter.0)) else {
            return self.unresolved(span, "for-loop iterator facts");
        };
        let for_match = self.synthetic_scope(e, Synthetic::ForMatch, ScopeData::Node, span)?;
        let for_arm = self.synthetic_scope(e, Synthetic::ForArm, ScopeData::Node, span)?;
        let for_arm_blocks = self.synthetic_scope(e, Synthetic::ForArm, ScopeData::Destruction, span)?;
        let iteration = self.synthetic_scope(e, Synthetic::Iteration, ScopeData::Node, span)?;
        let for_body = self.synthetic_scope(e, Synthetic::ForBody, ScopeData::Node, span)?;
        let for_body_stmt = self.synthetic_scope(e, Synthetic::ForBody, ScopeData::Destruction, span)?;

        // `IntoIterator::into_iter(<head>)`
        let head = self.mirror_expr(row.iter)?;
        let head_span = self.espan(row.iter);
        let into_ty = self.fn_def_ty(into_def, tcx.tys.list(into_args));
        let fun = self.push(Expr { kind: ExprKind::ZstLiteral { user_ty: None }, ty: into_ty, temp_scope: Some(for_match), bid: None, span: head_span });
        let into_call = self.push(Expr {
            kind: ExprKind::Call { ty: into_ty, fun, args: Box::new([head]), from_hir_call: true, fn_span: head_span },
            ty: iter_ty,
            temp_scope: Some(for_match), bid: None,
            span: head_span,
        });
        let into_scope = self.synthetic_scope(e, Synthetic::ForIntoIter, ScopeData::Node, span)?;
        let into_call = self.scope_wrap(e, into_scope, into_call);

        // `mut iter`
        let iter_var = self.synthetic_binding(for_arm);
        let iter_pat = self.binding_pat(iter_var, "iter", iter_ty, true, head_span);

        // `Iterator::next(&mut iter)`
        let opt_ty = self.method_output(next_def, tcx.tys.list(next_args), span)?;
        let next_ty = self.fn_def_ty(next_def, tcx.tys.list(next_args));
        let iter_ref = self.push(Expr { kind: ExprKind::VarRef { id: iter_var }, ty: iter_ty, temp_scope: Some(for_body_stmt), bid: None, span: head_span });
        let ref_mut = self.ref_ty(true, iter_ty);
        let borrow = self.push(Expr { kind: ExprKind::Borrow { borrow_kind: BorrowKind::Mut { kind: MutBorrowKind::Default }, arg: iter_ref }, ty: ref_mut, temp_scope: Some(for_body_stmt), bid: None, span: head_span });
        // the argument coerces to `&mut Self`: a (two-phase) reborrow of the borrow
        let deref = self.push(Expr { kind: ExprKind::Deref { arg: borrow }, ty: iter_ty, temp_scope: Some(for_body_stmt), bid: None, span: head_span });
        let reborrow = self.push(Expr { kind: ExprKind::Borrow { borrow_kind: BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow }, arg: deref }, ty: ref_mut, temp_scope: Some(for_body_stmt), bid: None, span: head_span });
        let next_fun = self.push(Expr { kind: ExprKind::ZstLiteral { user_ty: None }, ty: next_ty, temp_scope: Some(for_body_stmt), bid: None, span: head_span });
        let next_call = self.push(Expr {
            kind: ExprKind::Call { ty: next_ty, fun: next_fun, args: Box::new([reborrow]), from_hir_call: true, fn_span: head_span },
            ty: opt_ty,
            temp_scope: Some(for_body_stmt), bid: None,
            span: head_span,
        });
        let next_scope = self.synthetic_scope(e, Synthetic::ForNext, ScopeData::Node, span)?;
        let next_call = self.scope_wrap(e, next_scope, next_call);

        // `None => break`, `Some(<pat>) => <body>`
        self.breakables.push(Breakable { label: row.label, scope: iteration, is_block: false });
        let arms = (|| -> BuildResult<Vec<crate::mir::build::thir::ArmId>> {
            let none_idx = self.lang_variant(DefId(0), LangItem::OptionNone, span)?;
            let some_idx = self.lang_variant(DefId(0), LangItem::OptionSome, span)?;
            let none_pat = self.variant_pat(opt_ty, none_idx, Vec::new(), span)?;
            let brk = self.push(Expr { kind: ExprKind::Break { label: iteration, value: None }, ty: never, temp_scope: Some(for_body_stmt), bid: None, span });
            let none_body = self.push(Expr { kind: ExprKind::NeverToAny { source: brk }, ty: unit, temp_scope: Some(for_body_stmt), bid: None, span });
            let none_arm = self.thir.alloc_arm(Arm { pattern: Box::new(none_pat), guard: None, body: none_body, node: e.0, scope: for_body, span });

            self.pat_hint = self.pat_span(row.pat).lo;
            let user_pat = self.lower_pattern(row.pat)?;
            let some_pat = self.variant_pat(opt_ty, some_idx, vec![FieldPat { field: FieldIdx(0), pattern: user_pat }], span)?;
            let body = self.block_expr(e, row.body, unit)?;
            let some_arm = self.thir.alloc_arm(Arm { pattern: Box::new(some_pat), guard: None, body, node: e.0, scope: for_body, span });
            Ok(vec![none_arm, some_arm])
        })();
        self.breakables.pop();
        let arms = arms?;
        let match_next = self.push(Expr {
            kind: ExprKind::Match { scrutinee: next_call, arms: arms.into_boxed_slice(), match_source: MatchSource::ForLoopDesugar },
            ty: unit,
            temp_scope: Some(for_body_stmt), bid: None,
            span,
        });
        let stmt = self.thir.alloc_stmt(Stmt { kind: StmtKind::Expr { scope: for_body_stmt, expr: match_next } });
        let block = self.thir.alloc_block(Block { targeted_by_break: false, region_scope: for_arm_blocks, span, stmts: Box::new([stmt]), expr: None, safety_mode: BlockSafety::Safe });
        let loop_body = self.push(Expr { kind: ExprKind::Block { block }, ty: unit, temp_scope: Some(iteration), bid: None, span });
        let loop_expr = self.push(Expr { kind: ExprKind::Loop { body: loop_body }, ty: unit, temp_scope: Some(iteration), bid: None, span });
        let loop_scoped = self.push(Expr { kind: ExprKind::Scope { region_scope: iteration, node: e.0, value: loop_expr }, ty: unit, temp_scope: Some(iteration), bid: None, span });

        // `mut iter => loop { .. }`
        let iter_arm = self.thir.alloc_arm(Arm { pattern: Box::new(iter_pat), guard: None, body: loop_scoped, node: e.0, scope: for_arm, span });
        let outer = self.push(Expr {
            kind: ExprKind::Match { scrutinee: into_call, arms: Box::new([iter_arm]), match_source: MatchSource::ForLoopDesugar },
            ty: unit,
            temp_scope: self.temp_scope(e), bid: self.bid_scope(e),
            span,
        });
        let outer_scoped = self.push(Expr { kind: ExprKind::Scope { region_scope: for_match, node: e.0, value: outer }, ty: unit, temp_scope: self.temp_scope(e), bid: self.bid_scope(e), span });
        // `DropTemps(..)`
        Ok(self.mk(e, ExprKind::Use { source: outer_scoped }, expr_ty))
    }

    // ---------------------------------------------------------------- ?

    /// `expr?` (rustc_ast_lowering lower_expr_try):
    /// `match Try::branch(expr) { Continue(val) => val, Break(residual) => return FromResidual::from_residual(residual) }`
    pub(super) fn try_expr(&mut self, e: ast::ExprId, x: ast::ExprId, expr_ty: TyId) -> BuildResult<Expr> {
        let (tb, tcx, prog) = (self.tb(), self.tcx(), self.prog());
        let span = self.espan(e);
        let never = tcx.tys.never;
        let (Some(&(branch_def, branch_args, flow)), Some(&(from_def, from_args))) = (tb.try_branch.get(&e.0), tb.try_from_residual.get(&e.0)) else {
            return self.unresolved(span, "`?` facts");
        };
        let scope = self.node_scope(Node::Expr(e.0), span)?;
        let TyKind::Adt(flow_adt, flow_args, _) = *tcx.tys.kind(flow) else { return self.unresolved(span, "ControlFlow type") };
        let continue_idx = self.lang_variant(flow_adt, LangItem::ControlFlowContinue, span)?;
        let break_idx = self.lang_variant(flow_adt, LangItem::ControlFlowBreak, span)?;
        let field_ty = |variant: VariantIdx| tcx.field_ty(prog, self.root.cx.env, flow_adt, variant.0, 0, tcx.tys.list(flow_args));
        let (val_ty, residual_ty) = (field_ty(continue_idx), field_ty(break_idx));

        // `Try::branch(<expr>)`
        let arg = self.mirror_expr(x)?;
        let branch_ty = self.fn_def_ty(branch_def, tcx.tys.list(branch_args));
        let fun = self.push(self.mk(e, ExprKind::ZstLiteral { user_ty: None }, branch_ty));
        let scrutinee = self.push(self.mk(e, ExprKind::Call { ty: branch_ty, fun, args: Box::new([arg]), from_hir_call: true, fn_span: span }, flow));
        let branch_scope = self.synthetic_scope(e, Synthetic::TryBranch, ScopeData::Node, span)?;
        let scrutinee = self.scope_wrap(e, branch_scope, scrutinee);

        // `Continue(val) => val`
        let val_var = self.synthetic_binding(scope);
        let val_pat = self.binding_pat(val_var, "val", val_ty, false, span);
        let continue_pat = self.variant_pat(flow, continue_idx, vec![FieldPat { field: FieldIdx(0), pattern: val_pat }], span)?;
        let val = self.push(self.mk(e, ExprKind::VarRef { id: val_var }, val_ty));
        let continue_arm = self.thir.alloc_arm(Arm { pattern: Box::new(continue_pat), guard: None, body: val, node: e.0, scope, span });

        // `Break(residual) => return FromResidual::from_residual(residual)`
        let residual_var = self.synthetic_binding(scope);
        let residual_pat = self.binding_pat(residual_var, "residual", residual_ty, false, span);
        let break_pat = self.variant_pat(flow, break_idx, vec![FieldPat { field: FieldIdx(0), pattern: residual_pat }], span)?;
        let residual = self.push(self.mk(e, ExprKind::VarRef { id: residual_var }, residual_ty));
        let from_ty = self.fn_def_ty(from_def, tcx.tys.list(from_args));
        let from_ret = self.method_output(from_def, tcx.tys.list(from_args), span)?;
        let from_fun = self.push(self.mk(e, ExprKind::ZstLiteral { user_ty: None }, from_ty));
        let from_call = self.push(self.mk(e, ExprKind::Call { ty: from_ty, fun: from_fun, args: Box::new([residual]), from_hir_call: true, fn_span: span }, from_ret));
        let from_scope = self.synthetic_scope(e, Synthetic::TryFromResidual, ScopeData::Node, span)?;
        let from_call = self.scope_wrap(e, from_scope, from_call);
        let ret = self.push(self.mk(e, ExprKind::Return { value: Some(from_call) }, never));
        let ret_any = self.push(self.mk(e, ExprKind::NeverToAny { source: ret }, expr_ty));
        let break_arm = self.thir.alloc_arm(Arm { pattern: Box::new(break_pat), guard: None, body: ret_any, node: e.0, scope, span });

        Ok(self.mk(e, ExprKind::Match { scrutinee, arms: Box::new([break_arm, continue_arm]), match_source: MatchSource::TryDesugar }, expr_ty))
    }

    // ---------------------------------------------------------------- destructuring assignment

    /// `(a, b) = rhs` (rustc_ast_lowering lower_expr_assign):
    /// `{ let (lhs0, lhs1) = rhs; a = lhs0; b = lhs1; }`
    pub(super) fn destructuring_assign(&mut self, e: ast::ExprId, lhs: ast::ExprId, rhs: ast::ExprId, expr_ty: TyId) -> BuildResult<Expr> {
        let span = self.espan(e);
        let unit = self.unit_ty();
        let block_scope = self.synthetic_scope(e, Synthetic::AssignBlock, ScopeData::Node, span)?;
        let remainder_scope = self.synthetic_scope(e, Synthetic::AssignBlock, ScopeData::Remainder(0), span)?;
        let init_scope = self.synthetic_scope(e, Synthetic::AssignLet, ScopeData::Node, span)?;
        let rhs_ty = self.ety_adjusted(rhs)?;
        let initializer = self.mirror_expr(rhs)?;
        let mut leaves = Vec::new();
        let pattern = self.assignee_pat(lhs, rhs_ty, remainder_scope, &mut leaves)?;
        let mut stmts = vec![self.thir.alloc_stmt(Stmt {
            kind: StmtKind::Let { remainder_scope, init_scope, pattern: Box::new(pattern), initializer: Some(initializer), else_block: None, node: e.0, span },
        })];
        for (leaf, var, ty) in leaves {
            let leaf_span = self.espan(leaf);
            let scope = self.synthetic_scope(leaf, Synthetic::Assignment, ScopeData::Node, leaf_span)?;
            let target = self.mirror_expr(leaf)?;
            let value = self.push(self.mk_at(leaf, leaf_span, ExprKind::VarRef { id: var }, ty));
            let assign = self.push(self.mk_at(leaf, leaf_span, ExprKind::Assign { lhs: target, rhs: value }, unit));
            stmts.push(self.thir.alloc_stmt(Stmt { kind: StmtKind::Expr { scope, expr: assign } }));
        }
        let block = self.thir.alloc_block(Block { targeted_by_break: false, region_scope: block_scope, span, stmts: stmts.into_boxed_slice(), expr: None, safety_mode: BlockSafety::Safe });
        Ok(self.mk(e, ExprKind::Block { block }, expr_ty))
    }

    fn is_rest_assignee(&self, x: ast::ExprId) -> bool {
        let ast = self.ast();
        let x = super::strip_paren(ast, x);
        matches!(ast.expr(x).kind, A::Range(ast::OptExprId::NONE, ast::OptExprId::NONE, false))
    }

    /// The pattern the assignee `a` of type `ty` destructures with; each plain assignee becomes a
    /// fresh by-value binding recorded in `leaves`.
    fn assignee_pat(&mut self, a: ast::ExprId, ty: TyId, scope: ScopeId, leaves: &mut Vec<(ast::ExprId, LocalVarId, TyId)>) -> BuildResult<Pat> {
        let (ast, tcx, prog) = (self.ast(), self.tcx(), self.prog());
        let span = self.espan(a);
        let node = |kind| Pat { ty, span, extra: None, kind };
        match ast.expr(a).kind {
            A::Paren(x) => self.assignee_pat(x, ty, scope, leaves),
            A::Underscore => Ok(node(PatKind::Wild)),
            A::Tuple(elems) => {
                let TyKind::Tuple(l) = *tcx.tys.kind(ty) else { return self.unresolved(span, "tuple assignee of a non-tuple type") };
                let tys = tcx.tys.list(l).to_vec();
                let xs = ast.list(elems);
                let rest = xs.iter().position(|&x| self.is_rest_assignee(x));
                let mut subpatterns = Vec::new();
                for (i, &x) in xs.iter().enumerate() {
                    if Some(i) == rest {
                        continue;
                    }
                    let fi = match rest {
                        Some(r) if i > r => tys.len() - (xs.len() - i),
                        _ => i,
                    };
                    let Some(&ft) = tys.get(fi) else { return self.unresolved(span, "tuple assignee arity") };
                    let pattern = self.assignee_pat(x, ft, scope, leaves)?;
                    subpatterns.push(FieldPat { field: FieldIdx(fi as u32), pattern });
                }
                Ok(node(PatKind::Leaf { subpatterns }))
            }
            A::Array(elems) => {
                let (elem, array_len) = match *tcx.tys.kind(ty) {
                    TyKind::Slice(e) => (e, None),
                    TyKind::Array(e, len) => match len.try_to_target_usize(&tcx.tys) {
                        Some(len) => (e, Some(len)),
                        None => return self.unresolved(span, "array length of an assignee"),
                    },
                    _ => return self.unresolved(span, "array assignee of a non-sequence type"),
                };
                let xs = ast.list(elems);
                let rest = xs.iter().position(|&x| self.is_rest_assignee(x));
                let (before, after) = match rest {
                    Some(r) => (&xs[..r], &xs[r + 1..]),
                    None => (xs, &xs[..0]),
                };
                let mut prefix = Vec::new();
                for &x in before {
                    prefix.push(self.assignee_pat(x, elem, scope, leaves)?);
                }
                let mut suffix = Vec::new();
                for &x in after {
                    suffix.push(self.assignee_pat(x, elem, scope, leaves)?);
                }
                let slice = rest.map(|r| {
                    let fixed = (before.len() + after.len()) as u64;
                    let sub_ty = match array_len {
                        Some(n) => tcx.tys.intern(TyKind::Array(elem, tcx.tys.const_usize(n.saturating_sub(fixed)))),
                        None => tcx.tys.intern(TyKind::Slice(elem)),
                    };
                    Box::new(Pat { ty: sub_ty, span: self.espan(xs[r]), extra: None, kind: PatKind::Wild })
                });
                let (prefix, suffix) = (prefix.into_boxed_slice(), suffix.into_boxed_slice());
                Ok(node(match array_len {
                    None => PatKind::Slice { prefix, slice, suffix },
                    Some(_) => PatKind::Array { prefix, slice, suffix },
                }))
            }
            A::Call(f, args) => {
                let Some(Res::Def(d)) = self.tb().res.get(&a.0).copied() else { return self.unresolved(span, "constructor assignee") };
                let _ = f;
                let TyKind::Adt(adt, gargs, _) = *tcx.tys.kind(ty) else { return self.unresolved(span, "constructor assignee of a non-ADT type") };
                let variant = if prog.def(d).kind == DefKind::Variant { prog.def(d).sub } else { 0 };
                let nf = self.adt_variant_fields(adt, variant);
                let xs = ast.list(args);
                let rest = xs.iter().position(|&x| self.is_rest_assignee(x));
                let mut subpatterns = Vec::new();
                for (i, &x) in xs.iter().enumerate() {
                    if Some(i) == rest {
                        continue;
                    }
                    let fi = match rest {
                        Some(r) if i > r => nf - (xs.len() - i),
                        _ => i,
                    };
                    let ft = self.field_type(adt, variant, fi as u32, gargs);
                    let pattern = self.assignee_pat(x, ft, scope, leaves)?;
                    subpatterns.push(FieldPat { field: FieldIdx(fi as u32), pattern });
                }
                self.lower_variant_or_leaf(crate::typeck::user_types::UserTypeKey::Expr(a.0), ty, Some(d), subpatterns, span)
            }
            A::Struct(row) => {
                let Some(Res::Def(d)) = self.tb().res.get(&a.0).copied() else { return self.unresolved(span, "struct assignee") };
                let TyKind::Adt(adt, gargs, _) = *tcx.tys.kind(ty) else { return self.unresolved(span, "struct assignee of a non-ADT type") };
                let variant = if prog.def(d).kind == DefKind::Variant { prog.def(d).sub } else { 0 };
                let mut subpatterns = Vec::new();
                for f in ast.list(ast.row(row).fields) {
                    let Some(fi) = self.field_index(adt, variant, f.name) else { return self.unresolved(span, "unknown field in a struct assignee") };
                    let ft = self.field_type(adt, variant, fi, gargs);
                    let pattern = self.assignee_pat(f.expr, ft, scope, leaves)?;
                    subpatterns.push(FieldPat { field: FieldIdx(fi), pattern });
                }
                self.lower_variant_or_leaf(crate::typeck::user_types::UserTypeKey::Expr(a.0), ty, Some(d), subpatterns, span)
            }
            _ => {
                let var = self.synthetic_binding(scope);
                leaves.push((a, var, ty));
                Ok(self.binding_pat(var, "lhs", ty, false, span))
            }
        }
    }

    fn field_type(&self, adt: DefId, variant: u32, field: u32, gargs: crate::types::TyList) -> TyId {
        let tcx = self.tcx();
        self.normalize(tcx.field_ty(self.prog(), self.root.cx.env, adt, variant, field, tcx.tys.list(gargs)))
    }

    // ---------------------------------------------------------------- await

    /// `x.await` (rustc_ast_lowering lower_expr_await):
    /// ```text
    /// match IntoFuture::into_future(<x>) {
    ///     mut __awaitee => loop {
    ///         match unsafe { Future::poll(Pin::new_unchecked(&mut __awaitee), get_context(task_context)) } {
    ///             Poll::Ready(result) => break result,
    ///             Poll::Pending => {}
    ///         }
    ///         task_context = yield ();
    ///     }
    /// }
    /// ```
    /// (the `unsafe` block carries no scope in Relax's tree and no safety checking runs on THIR).
    pub(super) fn await_expr(&mut self, e: ast::ExprId, x: ast::ExprId, expr_ty: TyId) -> BuildResult<Expr> {
        let tcx = self.tcx();
        let span = self.espan(e);
        let (unit, never) = (self.unit_ty(), tcx.tys.never);
        let Some(task_context) = self.task_context else { return self.unresolved(span, "await outside a coroutine") };
        let resume_ty = self.resume_ty(span)?;
        let item = |this: &Self, item: LangItem| match tcx.lang_items.get(item) {
            Some(def) => Ok(def),
            None => this.unresolved(span, "await lang item"),
        };
        let (into_future, poll, new_unchecked, get_context) = (
            item(self, LangItem::IntoFutureIntoFuture)?,
            item(self, LangItem::FuturePoll)?,
            item(self, LangItem::PinNewUnchecked)?,
            item(self, LangItem::GetContext)?,
        );
        let arm_scope = self.synthetic_scope(e, Synthetic::AwaitArm, ScopeData::Node, span)?;
        let loop_scope = self.synthetic_scope(e, Synthetic::AwaitLoop, ScopeData::Node, span)?;
        let body_scope = self.synthetic_scope(e, Synthetic::AwaitBody, ScopeData::Node, span)?;
        let poll_scope = self.synthetic_scope(e, Synthetic::AwaitPoll, ScopeData::Node, span)?;
        let ready_scope = self.synthetic_scope(e, Synthetic::AwaitReady, ScopeData::Node, span)?;
        let pending_scope = self.synthetic_scope(e, Synthetic::AwaitPending, ScopeData::Node, span)?;
        let yield_scope = self.synthetic_scope(e, Synthetic::AwaitYield, ScopeData::Node, span)?;

        // `IntoFuture::into_future(<x>)`
        let operand_ty = self.ety_adjusted(x)?;
        let operand = self.mirror_expr(x)?;
        let into_ty = self.fn_def_ty(into_future, &[operand_ty]);
        let awaitee_ty = self.method_output(into_future, &[operand_ty], span)?;
        let outer_temp = self.temp_scope(e);
        let fun = self.push(Expr { kind: ExprKind::ZstLiteral { user_ty: None }, ty: into_ty, temp_scope: outer_temp, bid: None, span });
        let scrutinee = self.push(Expr {
            kind: ExprKind::Call { ty: into_ty, fun, args: Box::new([operand]), from_hir_call: true, fn_span: span },
            ty: awaitee_ty,
            temp_scope: outer_temp, bid: None,
            span,
        });
        let into_scope = self.synthetic_scope(e, Synthetic::AwaitIntoFuture, ScopeData::Node, span)?;
        let scrutinee = self.scope_wrap(e, into_scope, scrutinee);

        // `mut __awaitee`
        let awaitee = self.synthetic_binding(arm_scope);
        let awaitee_pat = self.binding_pat(awaitee, "__awaitee", awaitee_ty, true, span);

        // `Future::poll(Pin::new_unchecked(&mut __awaitee), get_context(task_context))`
        let at = Some(poll_scope);
        let ref_mut = self.ref_ty(true, awaitee_ty);
        let awaitee_ref = self.push(Expr { kind: ExprKind::VarRef { id: awaitee }, ty: awaitee_ty, temp_scope: at, bid: None, span });
        let borrow = self.push(Expr { kind: ExprKind::Borrow { borrow_kind: BorrowKind::Mut { kind: MutBorrowKind::Default }, arg: awaitee_ref }, ty: ref_mut, temp_scope: at, bid: None, span });
        let new_ty = self.fn_def_ty(new_unchecked, &[ref_mut]);
        let pinned_ty = self.method_output(new_unchecked, &[ref_mut], span)?;
        let new_fun = self.push(Expr { kind: ExprKind::ZstLiteral { user_ty: None }, ty: new_ty, temp_scope: at, bid: None, span });
        let pinned = self.push(Expr { kind: ExprKind::Call { ty: new_ty, fun: new_fun, args: Box::new([borrow]), from_hir_call: true, fn_span: span }, ty: pinned_ty, temp_scope: at, bid: None, span });
        let pinned_scope = self.synthetic_scope(e, Synthetic::AwaitNewUnchecked, ScopeData::Node, span)?;
        let pinned = self.scope_wrap(e, pinned_scope, pinned);
        let ctx_arg = self.push(Expr { kind: ExprKind::VarRef { id: task_context }, ty: resume_ty, temp_scope: at, bid: None, span });
        let ctx_ty = self.fn_def_ty(get_context, &[]);
        let context_ty = self.method_output(get_context, &[], span)?;
        let ctx_fun = self.push(Expr { kind: ExprKind::ZstLiteral { user_ty: None }, ty: ctx_ty, temp_scope: at, bid: None, span });
        let context = self.push(Expr { kind: ExprKind::Call { ty: ctx_ty, fun: ctx_fun, args: Box::new([ctx_arg]), from_hir_call: true, fn_span: span }, ty: context_ty, temp_scope: at, bid: None, span });
        let context_scope = self.synthetic_scope(e, Synthetic::AwaitGetContext, ScopeData::Node, span)?;
        let context = self.scope_wrap(e, context_scope, context);
        let poll_ty = self.fn_def_ty(poll, &[awaitee_ty]);
        let poll_result_ty = self.method_output(poll, &[awaitee_ty], span)?;
        let poll_fun = self.push(Expr { kind: ExprKind::ZstLiteral { user_ty: None }, ty: poll_ty, temp_scope: at, bid: None, span });
        let poll_call = self.push(Expr { kind: ExprKind::Call { ty: poll_ty, fun: poll_fun, args: Box::new([pinned, context]), from_hir_call: true, fn_span: span }, ty: poll_result_ty, temp_scope: at, bid: None, span });
        let poll_call_scope = self.synthetic_scope(e, Synthetic::AwaitPollCall, ScopeData::Node, span)?;
        let poll_call = self.scope_wrap(e, poll_call_scope, poll_call);

        // `Poll::Ready(result) => break result`, `Poll::Pending => {}`
        let ready_idx = self.lang_variant(DefId(0), LangItem::PollReady, span)?;
        let pending_idx = self.lang_variant(DefId(0), LangItem::PollPending, span)?;
        let result = self.synthetic_binding(ready_scope);
        let result_pat = self.binding_pat(result, "result", expr_ty, false, span);
        let ready_pat = self.variant_pat(poll_result_ty, ready_idx, vec![FieldPat { field: FieldIdx(0), pattern: result_pat }], span)?;
        let result_ref = self.push(Expr { kind: ExprKind::VarRef { id: result }, ty: expr_ty, temp_scope: Some(ready_scope), bid: None, span });
        let brk = self.push(Expr { kind: ExprKind::Break { label: loop_scope, value: Some(result_ref) }, ty: never, temp_scope: Some(ready_scope), bid: None, span });
        let ready_body = self.push(Expr { kind: ExprKind::NeverToAny { source: brk }, ty: unit, temp_scope: Some(ready_scope), bid: None, span });
        let ready_arm = self.thir.alloc_arm(Arm { pattern: Box::new(ready_pat), guard: None, body: ready_body, node: e.0, scope: ready_scope, span });
        let pending_pat = self.variant_pat(poll_result_ty, pending_idx, Vec::new(), span)?;
        let pending_body = self.push(Expr { kind: ExprKind::Tuple { fields: Box::new([]) }, ty: unit, temp_scope: Some(pending_scope), bid: None, span });
        let pending_arm = self.thir.alloc_arm(Arm { pattern: Box::new(pending_pat), guard: None, body: pending_body, node: e.0, scope: pending_scope, span });
        let inner_match = self.push(Expr {
            kind: ExprKind::Match { scrutinee: poll_call, arms: Box::new([ready_arm, pending_arm]), match_source: MatchSource::AwaitDesugar },
            ty: unit,
            temp_scope: at, bid: None,
            span,
        });
        let poll_stmt = self.thir.alloc_stmt(Stmt { kind: StmtKind::Expr { scope: poll_scope, expr: inner_match } });

        // `task_context = yield ();`
        let yt = Some(yield_scope);
        let unit_value = self.push(Expr { kind: ExprKind::Tuple { fields: Box::new([]) }, ty: unit, temp_scope: yt, bid: None, span });
        let yielded = self.push(Expr { kind: ExprKind::Yield { value: unit_value }, ty: resume_ty, temp_scope: yt, bid: None, span });
        let target = self.push(Expr { kind: ExprKind::VarRef { id: task_context }, ty: resume_ty, temp_scope: yt, bid: None, span });
        let assign = self.push(Expr { kind: ExprKind::Assign { lhs: target, rhs: yielded }, ty: unit, temp_scope: yt, bid: None, span });
        let yield_stmt = self.thir.alloc_stmt(Stmt { kind: StmtKind::Expr { scope: yield_scope, expr: assign } });

        // `loop { .. }`
        let block = self.thir.alloc_block(Block { targeted_by_break: false, region_scope: body_scope, span, stmts: Box::new([poll_stmt, yield_stmt]), expr: None, safety_mode: BlockSafety::Safe });
        let loop_body = self.push(Expr { kind: ExprKind::Block { block }, ty: unit, temp_scope: Some(loop_scope), bid: None, span });
        let loop_expr = self.push(Expr { kind: ExprKind::Loop { body: loop_body }, ty: expr_ty, temp_scope: Some(loop_scope), bid: None, span });
        let loop_scoped = self.push(Expr { kind: ExprKind::Scope { region_scope: loop_scope, node: e.0, value: loop_expr }, ty: expr_ty, temp_scope: Some(arm_scope), bid: None, span });

        // `mut __awaitee => loop { .. }`
        let awaitee_arm = self.thir.alloc_arm(Arm { pattern: Box::new(awaitee_pat), guard: None, body: loop_scoped, node: e.0, scope: arm_scope, span });
        Ok(self.mk(e, ExprKind::Match { scrutinee, arms: Box::new([awaitee_arm]), match_source: MatchSource::AwaitDesugar }, expr_ty))
    }

    // ---------------------------------------------------------------- builtin macros

    pub(super) fn mac_expr(&mut self, e: ast::ExprId, m: ast::MacCall, expr_ty: TyId) -> BuildResult<Expr> {
        let (ast, file) = (self.ast(), self.file());
        let span = self.espan(e);
        let name = ast.segs(m.path.segs).last().map(|s| file.text(s.name)).unwrap_or("");
        if name == "cfg" {
            // MIR building does not prune on the value (borrowck sees both branches)
            return Ok(self.lit_bool(e, false, span));
        }
        if m.args == ast::NO_MAC_ARGS {
            return self.unresolved(span, "macro without parsed arguments");
        }
        let margs = *ast.mac_arg(m.args);
        let xs: Vec<ast::ExprId> = ast.list(margs.exprs).to_vec();
        // `debug_assert*!(..)` is `if cfg!(debug_assertions) { assert*!(..) }`
        let debug = name.starts_with("debug_assert");
        let base = if debug { &name["debug_".len()..] } else { name };
        let inner_ty = if debug { self.unit_ty() } else { expr_ty };
        let inner = match base {
            "matches" => return self.matches_macro(e, &xs, margs.pat, margs.guard, expr_ty),
            "assert" => {
                let Some(&cond) = xs.first() else { return self.unresolved(span, "assert without a condition") };
                let branch = self.synthetic_scope(e, Synthetic::MacroBranch, ScopeData::Destruction, span)?;
                let branch_block = self.synthetic_scope(e, Synthetic::MacroBranch, ScopeData::Node, span)?;
                let stmt_scope = self.synthetic_scope(e, Synthetic::MacroStatement, ScopeData::Node, span)?;
                let cond = self.mirror_expr(cond)?;
                let not = self.push(self.mk(e, ExprKind::Unary { op: crate::mir::UnOp::Not, arg: cond }, self.bool_ty()));
                let unit = self.unit_ty();
                let then = self.fmt_block(e, &xs[1..], stmt_scope, branch_block, true, unit)?;
                self.mk(e, ExprKind::If { if_then_scope: branch, cond: not, then, else_opt: None }, inner_ty)
            }
            "assert_eq" | "assert_ne" => {
                let stmt_scope = self.synthetic_scope(e, Synthetic::MacroStatement, ScopeData::Node, span)?;
                let blocks = self.synthetic_scope(e, Synthetic::MacroStatement, ScopeData::Destruction, span)?;
                let block = self.fmt_block(e, &xs, stmt_scope, blocks, false, inner_ty)?;
                self.thir.exprs[block.0 as usize].clone()
            }
            "print" | "println" | "eprint" | "eprintln" | "panic" | "unreachable" | "todo" | "unimplemented" => {
                let diverges = matches!(base, "panic" | "unreachable" | "todo" | "unimplemented");
                let stmt_scope = self.synthetic_scope(e, Synthetic::MacroStatement, ScopeData::Node, span)?;
                let blocks = self.synthetic_scope(e, Synthetic::MacroStatement, ScopeData::Destruction, span)?;
                let block = self.fmt_block(e, &xs, stmt_scope, blocks, diverges, inner_ty)?;
                self.thir.exprs[block.0 as usize].clone()
            }
            _ => return self.unresolved(span, "unsupported builtin macro"),
        };
        if !debug {
            return Ok(inner);
        }
        let unit = self.unit_ty();
        let branch = self.synthetic_scope(e, Synthetic::DebugBranch, ScopeData::Destruction, span)?;
        let branch_block = self.synthetic_scope(e, Synthetic::DebugBranch, ScopeData::Node, span)?;
        let cond = self.push(self.lit_bool(e, true, span));
        let inner = self.push(inner);
        let block = self.thir.alloc_block(Block { targeted_by_break: false, region_scope: branch_block, span, stmts: Box::new([]), expr: Some(inner), safety_mode: BlockSafety::Safe });
        let then = self.push(self.mk(e, ExprKind::Block { block }, unit));
        Ok(self.mk(e, ExprKind::If { if_then_scope: branch, cond, then, else_opt: None }, expr_ty))
    }

    /// `{ &arg; &arg; ..; <unreachable>? }`: the format arguments are borrowed (`format_args!` takes
    /// `&arg`), by statement; a string literal is not a place. A panicking macro ends in `Unreachable`.
    fn fmt_block(&mut self, e: ast::ExprId, args: &[ast::ExprId], stmt_scope: ScopeId, blocks: ScopeId, diverges: bool, ty: TyId) -> BuildResult<ExprId> {
        let (tb, ast) = (self.tb(), self.ast());
        let span = self.espan(e);
        let mut stmts = Vec::new();
        let mut borrow_stmt = |this: &mut Self, arg: ExprId, arg_ty: TyId, arg_span: Span| {
            let ref_ty = this.ref_ty(false, arg_ty);
            let borrow = this.push(Expr { kind: ExprKind::Borrow { borrow_kind: BorrowKind::Shared, arg }, ty: ref_ty, temp_scope: Some(stmt_scope), bid: None, span: arg_span });
            let _ = arg_span;
            stmts.push(this.thir.alloc_stmt(Stmt { kind: StmtKind::Expr { scope: stmt_scope, expr: borrow } }));
        };
        for &x in args {
            if matches!(ast.expr(super::strip_paren(ast, x)).kind, A::Lit(_)) {
                continue;
            }
            let arg_ty = self.ety_adjusted(x)?;
            let arg = self.mirror_expr(x)?;
            borrow_stmt(self, arg, arg_ty, self.espan(x));
        }
        // inline `{name}` arguments
        let named: Vec<(crate::program::Sym, crate::typeck::Res)> = tb.fmt_named.iter().filter(|((node, _), _)| *node == e.0).map(|((_, sym), res)| (*sym, *res)).collect();
        for (_, res) in named {
            if let crate::typeck::Res::Local(index) = res {
                let var = LocalVarId(index);
                let ty = tb.locals[index as usize].ty;
                let kind = if self.is_upvar(index) { ExprKind::UpvarRef { var } } else { ExprKind::VarRef { id: var } };
                let arg = self.push(Expr { kind, ty, temp_scope: Some(stmt_scope), bid: None, span });
                borrow_stmt(self, arg, ty, span);
            }
        }
        let tail = if diverges {
            let never = self.tcx().tys.never;
            Some(self.push(Expr { kind: ExprKind::Unreachable, ty: never, temp_scope: Some(stmt_scope), bid: None, span }))
        } else {
            None
        };
        let block = self.thir.alloc_block(Block { targeted_by_break: false, region_scope: blocks, span, stmts: stmts.into_boxed_slice(), expr: tail, safety_mode: BlockSafety::Safe });
        Ok(self.push(self.mk(e, ExprKind::Block { block }, ty)))
    }

    /// `matches!(x, pat if guard)` is `match x { pat if guard => true, _ => false }`.
    fn matches_macro(&mut self, e: ast::ExprId, xs: &[ast::ExprId], pat: Option<ast::PatId>, guard: Option<ast::ExprId>, expr_ty: TyId) -> BuildResult<Expr> {
        let span = self.espan(e);
        let (Some(&x), Some(pat)) = (xs.first(), pat) else { return self.unresolved(span, "matches! arguments") };
        let arm_scope = self.synthetic_scope(e, Synthetic::MacroArm, ScopeData::Node, span)?;
        let scrutinee_ty = self.ety_adjusted(x)?;
        let scrutinee = self.mirror_expr(x)?;
        self.pat_hint = span.lo;
        let pattern = Box::new(self.lower_pattern(pat)?);
        let guard = match guard {
            Some(g) => Some(self.mirror_expr(g)?),
            None => None,
        };
        let yes = self.push(self.lit_bool(e, true, span));
        let first = self.thir.alloc_arm(Arm { pattern, guard, body: yes, node: e.0, scope: arm_scope, span });
        let no = self.push(self.lit_bool(e, false, span));
        let wild = Pat { ty: scrutinee_ty, span, extra: None, kind: PatKind::Wild };
        let second = self.thir.alloc_arm(Arm { pattern: Box::new(wild), guard: None, body: no, node: e.0, scope: arm_scope, span });
        Ok(self.mk(e, ExprKind::Match { scrutinee, arms: Box::new([first, second]), match_source: MatchSource::Normal }, expr_ty))
    }
}
