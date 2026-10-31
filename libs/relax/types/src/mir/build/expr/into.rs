//! rustc_mir_build/src/builder/expr/into.rs. Owner: lane E.
//! Deviations (documented stops, each a `BuildError`): `.use` of a non-Copy value (needs the
//! `UseCloned` query), the `write_via_move` / `write_box_via_move` intrinsics, struct-update
//! default fields, inline asm, `#[loop_match]`/`become` (not in this THIR).
use super::category::{Category, RvalueFunc};
use crate::mir::build::builder::{BlockFrame, Builder};
use crate::mir::build::cfg::{BlockAnd, BlockAndExtension};
use crate::mir::build::matches::DeclareLetBindings;
use crate::mir::build::thir::{AdtExpr, AdtExprBase, ExprId, ExprKind, FruInfo, LogicalOp};
use crate::mir::build::{BuildResult, UnsupportedReason};
use crate::mir::{
    AggregateKind, BasicBlock, BorrowKind, CanonicalUserTypeAnnotation, Const, ConstOperand, ConstValue, FieldIdx, LocalInfo, Mutability,
    Operand, Place, RawPtrKind, Rvalue, Scalar, SourceInfo, Span, TerminatorKind, UnwindAction, UserTypeAnnotationIndex,
};
use crate::types::TyKind;

macro_rules! unpack {
    ($b:ident, $e:expr) => {{
        let BlockAnd(nb, v) = $e?;
        $b = nb;
        v
    }};
}

impl Builder<'_> {
    /// into.rs:`expr_into_dest`. Compile `expr`, storing the result into `destination`, which is
    /// assumed to be uninitialized.
    pub fn expr_into_dest(&mut self, destination: Place, mut block: BasicBlock, expr_id: ExprId) -> BuildResult<BlockAnd<()>> {
        let expr = &self.thir.thir[expr_id];
        let (expr_span, expr_ty, kind) = (expr.span, expr.ty, expr.kind.clone());
        let source_info = self.source_info(expr_span);
        let tcx = self.root.cx.tcx;

        let expr_is_block_or_scope = matches!(kind, ExprKind::Block { .. } | ExprKind::Scope { .. });
        if !expr_is_block_or_scope {
            self.block_context.push(BlockFrame::SubExpr);
        }

        let block_and: BuildResult<BlockAnd<()>> = (|| match kind {
            ExprKind::Scope { region_scope, value, .. } => {
                self.in_scope((region_scope, source_info), |this| this.expr_into_dest(destination, block, value))
            }
            ExprKind::Block { block: ast_block } => self.ast_block(destination, block, ast_block, source_info),
            ExprKind::Match { scrutinee, ref arms, .. } => {
                let scrutinee_span = self.thir.thir[scrutinee].span;
                self.match_expr(destination, block, scrutinee, arms, expr_span, scrutinee_span)
            }
            ExprKind::If { cond, then, else_opt, if_then_scope } => {
                let then_span = self.thir.thir[then].span;
                let then_source_info = self.source_info(then_span);
                let condition_scope = self.local_scope();
                let dest = destination.clone();

                let BlockAnd(then_blk, else_blk0) = self.in_scope((if_then_scope, then_source_info), |this| {
                    let source_info = if this.is_let(cond) {
                        let variable_scope = this.new_source_scope(then_span, None);
                        this.source_scope = variable_scope;
                        SourceInfo { span: then_span, scope: variable_scope }
                    } else {
                        this.source_info(then_span)
                    };

                    // Lower the condition, and have it branch into `then` and `else` blocks.
                    let (then_block, else_block) = this.in_if_then_scope(condition_scope, then_span, |this| {
                        let then_blk = this
                            .then_else_break(block, cond, Some(condition_scope), source_info, DeclareLetBindings::Yes)?
                            .into_block();
                        // Lower the `then` arm into its block.
                        this.expr_into_dest(dest, then_blk, then)
                    })?;
                    Ok(then_block.and(else_block))
                })?;

                // If there is an `else` arm, lower it into `else_blk`.
                let else_blk = if let Some(else_expr) = else_opt {
                    self.expr_into_dest(destination.clone(), else_blk0, else_expr)?.into_block()
                } else {
                    // There is no `else` arm, so we know both arms have type `()`: the implicit `else {}`.
                    let correct_si = self.source_info(Span { file: expr_span.file, lo: expr_span.hi, hi: expr_span.hi });
                    self.cfg.push_assign_unit(else_blk0, correct_si, destination.clone(), tcx.tys.unit);
                    else_blk0
                };

                // The `then` and `else` arms meet up in a new block.
                let join_block = self.cfg.start_new_block();
                self.cfg.goto(then_blk, source_info, join_block);
                self.cfg.goto(else_blk, source_info, join_block);
                Ok(join_block.unit())
            }
            ExprKind::Let { .. } => {
                // After desugaring, `let` expressions only appear inside `if` expressions or `match`
                // guards, where their lowering handles them.
                Err(self.error(expr_span, UnsupportedReason::UnresolvedFact("let expression outside of if or match guard")))
            }
            ExprKind::NeverToAny { source } => {
                let source_expr = &self.thir.thir[source];
                let is_call = matches!(source_expr.kind, ExprKind::Call { .. });
                // (#66975) Source could be a const of type `!`, so has to exist in the generated MIR.
                let scope = self.local_temp_lifetime();
                let BlockAnd(b, _) = self.as_temp(block, scope, source, Mutability::Mut)?;
                block = b;
                // If the expression was a call we already have an unreachable block.
                if is_call {
                    Ok(block.unit())
                } else {
                    self.cfg.terminate(block, source_info, TerminatorKind::Unreachable);
                    Ok(self.cfg.start_new_block().unit())
                }
            }
            ExprKind::LogicalOp { op, lhs, rhs } => {
                let condition_scope = self.local_scope();
                let source_info = self.source_info(expr_span);

                // We first evaluate the left-hand side of the predicate ...
                let (then_block, else_block) = self.in_if_then_scope(condition_scope, expr_span, |this| {
                    this.then_else_break(block, lhs, Some(condition_scope), source_info, DeclareLetBindings::LetNotPermitted)
                })?;
                let (short_circuit, continuation, constant) = match op {
                    LogicalOp::And => (else_block, then_block, false),
                    LogicalOp::Or => (then_block, else_block, true),
                };
                // The control flow splits into a short-circuiting path (assigns `false` for `&&`, `true`
                // for `||`) and a continuation path evaluating `rhs`.
                let bool_const = ConstOperand {
                    span: expr_span,
                    user_ty: None,
                    const_: Const::Val(ConstValue::Scalar(Scalar::Int { bits: constant as u128, size: 1 }), tcx.tys.bool_),
                };
                self.cfg.push_assign(short_circuit, source_info, destination.clone(), Rvalue::Use(Operand::Constant(Box::new(bool_const))));
                let rhs_block = self.expr_into_dest(destination, continuation, rhs)?.into_block();
                let target = self.cfg.start_new_block();
                self.cfg.goto(rhs_block, source_info, target);
                self.cfg.goto(short_circuit, source_info, target);
                Ok(target.unit())
            }
            ExprKind::Loop { body } => {
                // [block] -> [loop_block] -> [body_block] -/eval. body/-> [body_block_end] -> [loop_block]
                // The false link makes borrowck consider unwinds through the body.
                let loop_block = self.cfg.start_new_block();
                self.cfg.goto(block, source_info, loop_block);

                self.in_breakable_scope(Some(loop_block), destination, expr_span, move |this| {
                    let body_block = this.cfg.start_new_block();
                    this.cfg.terminate(loop_block, source_info, TerminatorKind::FalseUnwind { real_target: body_block, unwind: UnwindAction::Continue });
                    this.diverge_from(loop_block)?;

                    // The "return" value of the loop body must always be a unit: a unit temporary is its destination.
                    let tmp = this.get_unit_temp();
                    let body_block_end = this.expr_into_dest(tmp, body_block, body)?.into_block();
                    this.cfg.goto(body_block_end, source_info, loop_block);

                    // Loops are only exited by `break` expressions.
                    Ok(None)
                })
            }
            ExprKind::Call { ty: _, fun, ref args, from_hir_call, fn_span } => {
                let BlockAnd(b, fun) = self.as_local_operand(block, fun)?;
                block = b;
                let mut operands = Vec::with_capacity(args.len());
                let mut arg_spans = Vec::with_capacity(args.len());
                for &arg in args.iter() {
                    let BlockAnd(b, op) = self.as_local_call_operand(block, arg)?;
                    block = b;
                    operands.push(op);
                    arg_spans.push(self.thir.thir[arg].span);
                }
                let success = self.cfg.start_new_block();
                self.record_operands_moved(&operands)?;
                self.cfg.terminate(block, source_info, TerminatorKind::Call {
                    func: fun,
                    args: operands,
                    unwind: UnwindAction::Continue,
                    destination,
                    // rustc: a call whose type is uninhabited has no target (the type check then expects an
                    // uninhabited signature output instead of relating it to the destination).
                    target: if tcx.tys.kind(expr_ty) == &crate::types::TyKind::Never { None } else { Some(success) },
                    call_source: if from_hir_call { crate::mir::CallSource::Normal } else { crate::mir::CallSource::OverloadedOperator },
                    fn_span,
                    arg_spans,
                });
                self.diverge_from(block)?;
                Ok(success.unit())
            }
            ExprKind::ByUse { expr, span: _ } => {
                let BlockAnd(b, place) = self.as_place(block, expr)?;
                block = b;
                let ty = self.local_place_ty(&place)?;
                let cx = &self.root.cx;
                if tcx.type_is_copy_modulo_regions(cx.prog, cx.eval, self.owner.owner, ty) {
                    self.cfg.push_assign(block, source_info, destination, Rvalue::Use(Operand::Copy(place)));
                    Ok(block.unit())
                } else {
                    Err(self.error(expr_span, UnsupportedReason::UnresolvedFact("`.use` of a value that is not Copy")))
                }
            }
            ExprKind::Use { source } => self.expr_into_dest(destination, block, source),
            ExprKind::Borrow { arg, borrow_kind } => {
                // Not in `as_rvalue`: borrows use `as_place`, so no `Rvalue` that stays valid across user code.
                let arg_place = match borrow_kind {
                    BorrowKind::Shared => unpack!(block, self.as_read_only_place(block, arg)),
                    _ => unpack!(block, self.as_place(block, arg)),
                };
                self.cfg.push_assign(block, source_info, destination, Rvalue::Ref(crate::mir::Region::ERASED, borrow_kind, arg_place));
                Ok(block.unit())
            }
            ExprKind::RawBorrow { mutability, arg } => {
                let place = match mutability {
                    Mutability::Not => unpack!(block, self.as_read_only_place(block, arg)),
                    Mutability::Mut => unpack!(block, self.as_place(block, arg)),
                };
                let kind = if mutability == Mutability::Mut { RawPtrKind::Mut } else { RawPtrKind::Const };
                self.cfg.push_assign(block, source_info, destination, Rvalue::RawPtr(kind, place));
                Ok(block.unit())
            }
            ExprKind::Adt(adt) => {
                let AdtExpr { adt_def, variant_index, args, user_ty, fields, base } = *adt;
                // See the notes for `ExprKind::Array` in `as_rvalue` and for `ExprKind::Borrow` above.
                let adt_t = tcx.adts.get(&adt_def);
                let is_union = adt_t.is_some_and(|a| a.is_union);
                let active_field_index = if is_union { fields.first().map(|f| f.name) } else { None };

                let scope = self.local_temp_lifetime();
                // first process the set of fields that were provided (evaluating them in the order given)
                let mut fields_map: Vec<(FieldIdx, Operand)> = Vec::with_capacity(fields.len());
                for f in fields.iter() {
                    let BlockAnd(b, op) = self.as_operand(block, scope, f.expr, LocalInfo::AggregateTemp, true)?;
                    block = b;
                    fields_map.push((f.name, op));
                }
                let provided = |n: FieldIdx| fields_map.iter().find(|(m, _)| *m == n).map(|(_, o)| o.clone());
                let n_fields = adt_t.and_then(|a| a.variants.get(variant_index.0 as usize)).map_or(0, |v| v.fields.len());
                let field_names = (0..n_fields as u32).map(FieldIdx);

                let operands: Vec<Operand> = match base {
                    AdtExprBase::None => field_names.filter_map(provided).collect(),
                    AdtExprBase::Base(FruInfo { base, field_types }) => {
                        let BlockAnd(b, place_builder) = self.as_place_builder(block, base)?;
                        block = b;
                        let mut ops = Vec::with_capacity(n_fields);
                        // We desugar FRU as we lower to MIR: each base-supplied field reads from the base.
                        for (n, ty) in field_names.zip(field_types.iter()) {
                            ops.push(match provided(n) {
                                Some(v) => v,
                                None => {
                                    let place = place_builder.clone().field(n, *ty).to_place(self);
                                    self.capture_operand(place)?
                                }
                            });
                        }
                        ops
                    }
                    AdtExprBase::DefaultFields(_) => {
                        return Err(self.error(expr_span, UnsupportedReason::UnresolvedFact("struct default field values")));
                    }
                };

                let user_ty = user_ty.map(|user_ty| {
                    self.canonical_user_type_annotations.push(CanonicalUserTypeAnnotation { span: source_info.span, user_ty, inferred_ty: expr_ty });
                    UserTypeAnnotationIndex(self.canonical_user_type_annotations.len() as u32 - 1)
                });
                let aggregate = Box::new(AggregateKind::Adt(adt_def, variant_index, args, user_ty, active_field_index));
                self.cfg.push_assign(block, source_info, destination, Rvalue::Aggregate(aggregate, operands));
                Ok(block.unit())
            }

            // These cases don't actually need a destination
            ExprKind::Assign { .. } | ExprKind::AssignOp { .. } => {
                block = self.stmt_expr(block, expr_id, None)?.into_block();
                self.cfg.push_assign_unit(block, source_info, destination, tcx.tys.unit);
                Ok(block.unit())
            }
            ExprKind::Continue { .. } | ExprKind::Break { .. } | ExprKind::Return { .. } => {
                block = self.stmt_expr(block, expr_id, None)?.into_block();
                // No assign, as these have type `!`.
                Ok(block.unit())
            }

            // Avoid creating a temporary
            ExprKind::VarRef { .. } | ExprKind::UpvarRef { .. } | ExprKind::PlaceTypeAscription { .. } | ExprKind::ValueTypeAscription { .. } => {
                let place = unpack!(block, self.as_place(block, expr_id));
                let operand = self.capture_operand(place)?;
                self.cfg.push_assign(block, source_info, destination, Rvalue::Use(operand));
                Ok(block.unit())
            }
            ExprKind::Index { .. } | ExprKind::Deref { .. } | ExprKind::Field { .. } => {
                // A "fake" temporary variable so that we check that the value is Sized (box exprs have no such check).
                if !destination.projection.is_empty() {
                    self.new_local(expr_ty, Mutability::Mut, crate::mir::LocalKind::Temp, LocalInfo::Boring, expr_span);
                }
                let place = unpack!(block, self.as_place(block, expr_id));
                let operand = self.capture_operand(place)?;
                self.cfg.push_assign(block, source_info, destination, Rvalue::Use(operand));
                Ok(block.unit())
            }

            // Control never continues (the end of a diverging builtin macro): no destination.
            ExprKind::Unreachable => {
                self.cfg.terminate(block, source_info, TerminatorKind::Unreachable);
                Ok(self.cfg.start_new_block().unit())
            }

            ExprKind::Yield { value } => {
                let scope = self.local_temp_lifetime();
                let BlockAnd(b, value) = self.as_operand(block, scope, value, LocalInfo::Boring, false)?;
                block = b;
                let resume = self.cfg.start_new_block();
                self.cfg.terminate(block, source_info, TerminatorKind::Yield { value, resume, resume_arg: destination, drop: None });
                self.coroutine_drop_cleanup(block)?;
                Ok(resume.unit())
            }

            // these are the cases that are more naturally handled by some other mode
            ExprKind::Unary { .. }
            | ExprKind::Binary { .. }
            | ExprKind::Cast { .. }
            | ExprKind::PointerCoercion { .. }
            | ExprKind::Repeat { .. }
            | ExprKind::Array { .. }
            | ExprKind::Tuple { .. }
            | ExprKind::Closure(..)
            | ExprKind::ConstBlock { .. }
            | ExprKind::Literal { .. }
            | ExprKind::NamedConst { .. }
            | ExprKind::NonHirLiteral { .. }
            | ExprKind::ZstLiteral { .. }
            | ExprKind::ConstParam { .. }
            | ExprKind::ThreadLocalRef(_)
            | ExprKind::StaticRef { .. } => {
                // Rvalue(Into) and Place kinds are handled above; otherwise we would recurse forever.
                debug_assert!(!matches!(Category::of(&kind), Some(Category::Rvalue(RvalueFunc::Into) | Category::Place)));
                let rvalue = unpack!(block, self.as_local_rvalue(block, expr_id));
                self.cfg.push_assign(block, source_info, destination, rvalue);
                Ok(block.unit())
            }
        })();

        if !expr_is_block_or_scope {
            let popped = self.block_context.pop();
            debug_assert!(popped.is_some());
        }
        block_and
    }

    fn is_let(&self, expr: ExprId) -> bool {
        match self.thir.thir[expr].kind {
            ExprKind::Let { .. } => true,
            ExprKind::Scope { value, .. } => self.is_let(value),
            _ => false,
        }
    }

    /// builder/mod.rs:`get_unit_temp`: a unit temporary used as the destination of statements.
    pub fn get_unit_temp(&mut self) -> Place {
        match &self.unit_temp {
            Some(tmp) => tmp.clone(),
            None => {
                let unit = self.root.cx.tcx.tys.unit;
                let tmp = self.temp(unit, self.body_span);
                self.unit_temp = Some(tmp.clone());
                tmp
            }
        }
    }

    fn local_place_ty(&self, place: &Place) -> BuildResult<crate::types::TyId> {
        let tcx = self.root.cx.tcx;
        let mut ty = crate::mir::PlaceTy::from_ty(self.local_decls[place.local.0 as usize].ty);
        for elem in &place.projection {
            ty = ty.projection_ty(tcx, elem).ok_or_else(|| self.error(Span::default(), UnsupportedReason::UnresolvedFact("ill-typed place projection")))?;
        }
        Ok(ty.ty)
    }
}

#[allow(dead_code)]
fn _kinds(_: &TyKind) {}

#[cfg(test)]
mod tests {
    use super::super::as_rvalue::tests::{builder, fixture, lit};
    use super::*;
    use crate::mir::build::thir::{Expr, ExprKind as K};

    #[test]
    fn literals_tuples_and_logical_ops_lower_into_a_destination() {
        let mut fx = fixture();
        let t = &fx.builder_parts.1.tys;
        let (i32_, bool_) = (t.i32_, t.bool_);
        let tuple_ty = t.intern(TyKind::Tuple(t.mk_list(&[i32_, i32_])));
        let a = lit(&mut fx, "42", i32_, false);
        let b = lit(&mut fx, "42", i32_, true);
        let tup = fx.thir.alloc_expr(Expr { kind: K::Tuple { fields: vec![a, b].into() }, ty: tuple_ty, temp_scope: None, bid: None, span: Span::default() });
        let (mut bl, _) = builder(fx);
        let dest = bl.temp(tuple_ty, Span::default());
        let start = bl.cfg.start_new_block();
        let BlockAnd(end, ()) = bl.expr_into_dest(dest.clone(), start, tup).unwrap();
        assert_eq!(end, start, "a tuple of constants needs no control flow");
        let stmts = &bl.cfg.block_data(start).statements;
        assert!(matches!(&stmts.last().unwrap().kind, crate::mir::StatementKind::Assign(p, Rvalue::Aggregate(k, ops))
            if *p == dest && matches!(**k, AggregateKind::Tuple) && ops.len() == 2));
        assert!(bl.block_context.is_empty(), "the sub-expression frame is popped");
        let _ = bool_;
    }

    fn unit_tuple(fx: &mut super::super::as_rvalue::tests::Fx) -> ExprId {
        let unit = fx.builder_parts.1.tys.unit;
        fx.thir.alloc_expr(Expr { kind: K::Tuple { fields: Vec::new().into() }, ty: unit, temp_scope: None, bid: None, span: Span::default() })
    }

    #[test]
    fn a_loop_jumps_back_to_its_head_and_has_no_normal_exit() {
        let mut fx = fixture();
        let unit = fx.builder_parts.1.tys.unit;
        let body = unit_tuple(&mut fx);
        let lp = fx.thir.alloc_expr(Expr { kind: K::Loop { body }, ty: unit, temp_scope: None, bid: None, span: Span::default() });
        let (mut bl, _) = builder(fx);
        let dest = bl.temp(unit, Span::default());
        let start = bl.cfg.start_new_block();
        let BlockAnd(exit, ()) = bl.expr_into_dest(dest, start, lp).unwrap();
        // start -> loop head (FalseUnwind to the body) -> body -> loop head.
        let head = match bl.cfg.block_data(start).terminator.as_ref().unwrap().kind { crate::mir::TerminatorKind::Goto { target } => target, ref k => panic!("{k:?}") };
        let body_block = match bl.cfg.block_data(head).terminator.as_ref().unwrap().kind { crate::mir::TerminatorKind::FalseUnwind { real_target, .. } => real_target, ref k => panic!("{k:?}") };
        assert!(matches!(bl.cfg.block_data(body_block).terminator.as_ref().unwrap().kind, crate::mir::TerminatorKind::Goto { target } if target == head), "the body ends by jumping back");
        assert!(bl.cfg.block_data(exit).terminator.is_none(), "the exit block is only reachable by `break`: nothing jumps to it here");
    }

    #[test]
    fn a_call_evaluates_function_and_arguments_then_terminates_with_a_call() {
        let mut fx = fixture();
        let i32_ = fx.builder_parts.1.tys.i32_;
        let unit = fx.builder_parts.1.tys.unit;
        let fun = fx.thir.alloc_expr(Expr { kind: K::ZstLiteral { user_ty: None }, ty: i32_, temp_scope: None, bid: None, span: Span::default() });
        let arg = lit(&mut fx, "42", i32_, false);
        let call = fx.thir.alloc_expr(Expr { kind: K::Call { ty: i32_, fun, args: vec![arg].into(), from_hir_call: true, fn_span: Span::default() }, ty: unit, temp_scope: None, bid: None, span: Span::default() });
        let (mut bl, _) = builder(fx);
        let dest = bl.temp(unit, Span::default());
        let start = bl.cfg.start_new_block();
        let BlockAnd(after, ()) = bl.expr_into_dest(dest.clone(), start, call).unwrap();
        let term = bl.cfg.block_data(start).terminator.as_ref().unwrap();
        match &term.kind {
            crate::mir::TerminatorKind::Call { args, destination, target, call_source, .. } => {
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0], Operand::Constant(_)), "a literal argument is a constant operand");
                assert_eq!(*destination, dest);
                assert_eq!(*target, Some(after), "the call returns to the block the lowering continues in");
                assert!(matches!(call_source, crate::mir::CallSource::Normal));
            }
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn an_adt_expression_orders_operands_by_field_not_by_the_written_order() {
        let mut fx = fixture();
        let (prog, tcx) = (fx.builder_parts.0, fx.builder_parts.1);
        let i32_ = tcx.tys.i32_;
        let p = (0..prog.defs.len()).map(|i| crate::program::DefId(i as u32)).find(|&d| prog.name(d) == "P").unwrap();
        let adt_ty = tcx.tys.intern(TyKind::Adt(p, tcx.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY));
        let neg = lit(&mut fx, "42", i32_, true);
        let pos = lit(&mut fx, "42", i32_, false);
        // `P { b: 42, a: -42 }`
        let adt = AdtExpr { adt_def: p, variant_index: crate::mir::VariantIdx(0), args: Vec::new(), user_ty: None,
            fields: vec![crate::mir::build::thir::FieldExpr { name: crate::mir::FieldIdx(1), expr: pos }, crate::mir::build::thir::FieldExpr { name: crate::mir::FieldIdx(0), expr: neg }].into(),
            base: AdtExprBase::None };
        let e = fx.thir.alloc_expr(Expr { kind: K::Adt(Box::new(adt)), ty: adt_ty, temp_scope: None, bid: None, span: Span::default() });
        let (mut bl, _) = builder(fx);
        let dest = bl.temp(adt_ty, Span::default());
        let start = bl.cfg.start_new_block();
        let BlockAnd(end, ()) = bl.expr_into_dest(dest.clone(), start, e).unwrap();
        assert_eq!(end, start);
        match &bl.cfg.block_data(start).statements.last().unwrap().kind {
            crate::mir::StatementKind::Assign(pl, Rvalue::Aggregate(k, ops)) => {
                assert_eq!(*pl, dest);
                assert!(matches!(**k, AggregateKind::Adt(d, v, ..) if d == p && v.0 == 0));
                let bits = |o: &Operand| match o { Operand::Constant(c) => match &c.const_ { crate::mir::Const::Val(crate::mir::ConstValue::Scalar(crate::mir::Scalar::Int { bits, .. }), _) => *bits, c => panic!("{c:?}") }, o => panic!("{o:?}") };
                assert_eq!(ops.iter().map(bits).collect::<Vec<_>>(), vec![0xFFFF_FFD6, 42], "field a (written second) comes first");
            }
            s => panic!("{s:?}"),
        }
    }
}
