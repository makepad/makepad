//! rustc_mir_build/src/builder/block.rs: THIR blocks and statements. Owner: lane B.
use super::builder::{BlockFrame, Builder, ForGuard};
use super::cfg::{BlockAnd, BlockAndExtension};
use super::matches::{DeclareLetBindings, ScheduleDrops};
use super::thir::{BlockId, ExprId, PatKind, Stmt, StmtId, StmtKind};
use super::{BuildResult, UnsupportedReason};
use crate::mir::{BasicBlock, BlockTailInfo, Place, SourceInfo, Span, TerminatorKind};
use crate::region_scope::ScopeId;
use crate::types::TyKind;

impl Builder<'_> {
    pub fn ast_block(&mut self, destination: Place, block: BasicBlock, ast_block: BlockId, source_info: SourceInfo) -> BuildResult<BlockAnd<()>> {
        let thir = self.thir;
        let b = &thir.thir[ast_block];
        let (region_scope, span, targeted_by_break, expr) = (b.region_scope, b.span, b.targeted_by_break, b.expr);
        let stmts = &b.stmts;
        self.in_scope((region_scope, source_info), move |this| {
            if targeted_by_break {
                this.in_breakable_scope(None, destination.clone(), span, |this| Ok(Some(this.ast_block_stmts(destination, block, span, stmts, expr, region_scope)?)))
            } else {
                this.ast_block_stmts(destination, block, span, stmts, expr, region_scope)
            }
        })
    }

    fn ast_block_stmts(
        &mut self,
        destination: Place,
        mut block: BasicBlock,
        span: Span,
        stmts: &[StmtId],
        expr: Option<ExprId>,
        region_scope: ScopeId,
    ) -> BuildResult<BlockAnd<()>> {
        let thir = self.thir;
        // This convoluted structure is to avoid using recursion as we walk down a list of statements. Basically, the
        // structure we get back is something like:
        //
        //    let x = <init> in {
        //       expr1;
        //       let y = <init> in {
        //           expr2;
        //           expr3;
        //           ...
        //       }
        //    }
        //
        // The let bindings are valid till the end of block so all we have to do is to pop all the let-scopes at the
        // end.
        //
        // First we build all the statements in the block.
        let mut let_scope_stack = Vec::with_capacity(8);
        let outer_source_scope = self.source_scope;
        let mut last_remainder_scope = region_scope;

        let source_info = self.source_info(span);
        for stmt in stmts {
            let Stmt { kind } = &thir.thir[*stmt];
            match kind {
                StmtKind::Expr { scope, expr } => {
                    self.block_context.push(BlockFrame::Statement { ignores_expr_result: true });
                    let si = (*scope, source_info);
                    let (scope, expr) = (*scope, *expr);
                    block = self.in_scope(si, |this| this.stmt_expr(block, expr, Some(scope)))?.into_block();
                }
                StmtKind::Let { remainder_scope, init_scope, pattern, initializer: Some(initializer), else_block: Some(else_block), .. } => {
                    // When lowering the statement `let <pat> = <expr> else { <else> };`, the `<else>` block is nested
                    // in the parent scope enclosing this statement. That scope is usually either the enclosing block
                    // scope, or the remainder scope of the last statement.
                    //
                    // We also introduce the (hidden) scope for the else block's pattern bindings. For let-else, the
                    // parent of `else` is the remainder scope of the *previous* statement.
                    let ignores_expr_result = matches!(pattern.kind, PatKind::Wild);
                    self.block_context.push(BlockFrame::Statement { ignores_expr_result });

                    // Lower the `else` block first because its parent scope is actually enclosing the rest of the `let`
                    // statement.
                    let else_block_span = thir.thir[*else_block].span;
                    let never = self.root.cx.tcx.tys.never;
                    let dummy_place = self.temp(never, else_block_span);
                    let failure_entry = self.cfg.start_new_block();
                    let si = self.source_info(else_block_span);
                    let failure_block = self.ast_block(dummy_place, failure_entry, *else_block, si)?.into_block();
                    let si = self.source_info(else_block_span);
                    self.cfg.terminate(failure_block, si, TerminatorKind::Unreachable);

                    // Declare the bindings, which may create a source scope.
                    let remainder_span = self.region_scope_span(*remainder_scope);
                    self.push_scope((*remainder_scope, source_info));
                    let_scope_stack.push(remainder_scope);

                    let visibility_scope = Some(self.new_source_scope(remainder_span, None));

                    let initializer_span = thir.thir[*initializer].span;
                    let scope = (*init_scope, source_info);
                    let (pattern, initializer, destination2) = (&**pattern, *initializer, destination.clone());
                    let failure_and_block = self.in_scope(scope, |this| {
                        this.declare_bindings(visibility_scope, remainder_span, pattern, None, Some((Some(&destination2), initializer_span)))?;
                        let (matching, failure) = this.in_if_then_scope(last_remainder_scope, else_block_span, |this| {
                            this.lower_let_expr(block, initializer, pattern, None, initializer_span, DeclareLetBindings::No)
                        })?;
                        Ok(matching.and(failure))
                    });
                    let failure = unpack!(block = failure_and_block);
                    self.cfg.goto(failure, source_info, failure_entry);

                    if let Some(source_scope) = visibility_scope {
                        self.source_scope = source_scope;
                    }
                    last_remainder_scope = *remainder_scope;
                }
                StmtKind::Let { init_scope, initializer: None, else_block: Some(_), .. } => {
                    return Err(self.error(self.region_scope_span(*init_scope), UnsupportedReason::Internal("initializer is missing, but else block is present in this let binding")));
                }
                StmtKind::Let { remainder_scope, init_scope, pattern, initializer, else_block: None, .. } => {
                    // Enter the remainder scope, i.e., the bindings' destruction scope.
                    let ignores_expr_result = matches!(pattern.kind, PatKind::Wild);
                    self.block_context.push(BlockFrame::Statement { ignores_expr_result });

                    // Enter the remainder scope, i.e., the bindings' destruction scope.
                    self.push_scope((*remainder_scope, source_info));
                    let_scope_stack.push(remainder_scope);

                    // Declare the bindings, which may create a source scope.
                    let remainder_span = self.region_scope_span(*remainder_scope);

                    let visibility_scope = Some(self.new_source_scope(remainder_span, None));

                    // Evaluate the initializer, if present.
                    let pattern = &**pattern;
                    if let Some(init) = *initializer {
                        let initializer_span = thir.thir[init].span;
                        let scope = (*init_scope, source_info);

                        block = self
                            .in_scope(scope, |this| {
                                this.declare_bindings(visibility_scope, remainder_span, pattern, None, Some((None, initializer_span)))?;
                                this.expr_into_pattern(block, pattern, init) // irrefutable pattern
                            })?
                            .into_block();
                    } else {
                        let scope = (*init_scope, source_info);
                        let _: BlockAnd<()> = self.in_scope(scope, |this| {
                            this.declare_bindings(visibility_scope, remainder_span, pattern, None, None)?;
                            Ok(block.unit())
                        })?;

                        self.visit_primary_bindings(pattern, &mut |this, node, span| {
                            this.storage_live_binding(block, node, span, false, ForGuard::OutsideGuard, ScheduleDrops::Yes)?;
                            this.schedule_drop_for_binding(node, span, ForGuard::OutsideGuard)
                        })?;
                    }

                    // Enter the visibility scope, after evaluating the initializer.
                    if let Some(source_scope) = visibility_scope {
                        self.source_scope = source_scope;
                    }
                    last_remainder_scope = *remainder_scope;
                }
            }

            let popped = self.block_context.pop();
            debug_assert!(matches!(popped, Some(BlockFrame::Statement { .. })));
        }
        // Then, the block may have an optional trailing expression which is a “return” value of the block, which is
        // stored into `destination`.
        let tcx = self.root.cx.tcx;
        let destination_ty = self.place_ty(&destination)?.ty;
        let is_unit = matches!(tcx.tys.kind(destination_ty), TyKind::Tuple(ts) if tcx.tys.list(*ts).is_empty());
        if let Some(expr_id) = expr {
            let expr_span = thir.thir[expr_id].span;
            let tail_result_is_ignored = is_unit || self.currently_ignores_tail_results();
            self.block_context.push(BlockFrame::TailExpr { info: BlockTailInfo { tail_result_is_ignored, span: expr_span } });

            block = self.expr_into_dest(destination, block, expr_id)?.into_block();
            let popped = self.block_context.pop();

            debug_assert!(matches!(popped, Some(BlockFrame::TailExpr { .. })));
        } else {
            // If a block has no trailing expression, then it is given an implicit return type. This return type is
            // usually `()`, unless the block is diverging, in which case the return type is `!`. For the unit type, we
            // need to actually return the unit, but in the case of `!`, no return value is required, as the block will
            // never return. Opaque types are the exception, as the block's return type will be an opaque type
            // whose hidden type is `()`.
            if is_unit || matches!(tcx.tys.kind(destination_ty), TyKind::Alias(_, a) if matches!(a.kind, crate::types::AliasTyKind::Opaque { .. })) {
                self.cfg.push_assign_unit(block, source_info, destination, tcx.tys.unit);
            }
        }
        // Finally, we pop all the let scopes before exiting out from the scope of block itself.
        for scope in let_scope_stack.into_iter().rev() {
            block = self.pop_scope((*scope, source_info), block)?.into_block();
        }
        // Restore the original source scope.
        self.source_scope = outer_source_scope;
        Ok(block.unit())
    }
}
