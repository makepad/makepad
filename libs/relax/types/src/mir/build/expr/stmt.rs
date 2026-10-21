//! rustc_mir_build/src/builder/expr/stmt.rs. Owner: lane B.
//! Deviations: no `become`/`#[loop_match]` (`ConstContinue`) statements (unstable; not in the THIR).
use crate::mir::build::builder::{Builder, BlockFrame, TempLifetime};
use crate::mir::build::cfg::{BlockAnd, BlockAndExtension};
use crate::mir::build::scope::BreakableTarget;
use crate::mir::build::thir::{ExprId, ExprKind};
use crate::mir::build::BuildResult;
use crate::mir::{BasicBlock, BlockTailInfo, Mutability, Operand};
use crate::region_scope::ScopeId;

impl Builder<'_> {
    /// Builds a block of MIR for the given statement expression. In addition to the block, it returns the unit
    /// result.
    pub fn stmt_expr(&mut self, mut block: BasicBlock, expr_id: ExprId, statement_scope: Option<ScopeId>) -> BuildResult<BlockAnd<()>> {
        let thir = self.thir;
        let expr = &thir.thir[expr_id];
        let expr_span = expr.span;
        let source_info = self.source_info(expr.span);
        match &expr.kind {
            ExprKind::Scope { region_scope, value, .. } => {
                let (region_scope, value) = (*region_scope, *value);
                self.in_scope((region_scope, source_info), |this| this.stmt_expr(block, value, statement_scope))
            }
            ExprKind::Assign { lhs, rhs } => {
                let (lhs, rhs) = (*lhs, *rhs);
                let lhs_expr = &thir.thir[lhs];

                // Note: we evaluate assignments right-to-left. This is better for borrowck interaction with overloaded
                // operators like +=, but it's easier to (generally) reason about.
                self.block_context.push(BlockFrame::SubExpr);

                // Generate better code for things that don't need to be dropped.
                if self.needs_drop(lhs_expr.ty) {
                    let rhs = unpack!(block = self.as_local_rvalue(block, rhs));
                    let lhs = unpack!(block = self.as_place(block, lhs));
                    block = self.build_drop_and_replace(block, lhs_expr.span, lhs, rhs)?.into_block();
                } else {
                    let rhs = unpack!(block = self.as_local_rvalue(block, rhs));
                    let lhs = unpack!(block = self.as_place(block, lhs));
                    self.cfg.push_assign(block, source_info, lhs, rhs);
                }

                self.block_context.pop();
                Ok(block.unit())
            }
            ExprKind::AssignOp { op, lhs, rhs } => {
                let (op, lhs, rhs) = (op.0, *lhs, *rhs);
                // FIXME(#28160) there is an interesting semantics question raised here -- should we "freeze" the
                // value of the lhs here? I'm inclined to think not, since it seems closer to the semantics of the
                // overloaded version, which takes `&mut self`. This only affects weird things like `x += {x += 1; x}`
                // -- is that equal to `x + (x + 1)` or `2*(x+1)`?
                let lhs_ty = thir.thir[lhs].ty;

                self.block_context.push(BlockFrame::SubExpr);

                // As above, RTL.
                let rhs = unpack!(block = self.as_local_operand(block, rhs));
                let lhs = unpack!(block = self.as_place(block, lhs));

                // we don't have to drop prior contents or anything because AssignOp is only legal for Copy types
                // (overloaded ops should be desugared into a call).
                let result = unpack!(block = self.build_binary_op(block, op, expr_span, lhs_ty, Operand::Copy(lhs.clone()), rhs));
                self.cfg.push_assign(block, source_info, lhs, result);

                self.block_context.pop();
                Ok(block.unit())
            }
            ExprKind::Continue { label } => self.break_scope(block, None, BreakableTarget::Continue(*label), source_info),
            ExprKind::Break { label, value } => self.break_scope(block, *value, BreakableTarget::Break(*label), source_info),
            ExprKind::Return { value } => self.break_scope(block, *value, BreakableTarget::Return, source_info),
            _ => {
                if statement_scope.is_none() {
                    return Err(self.error(expr_span, crate::mir::build::UnsupportedReason::Internal("stmt_expr on a general expression without a statement scope")));
                }

                // Issue #54382: When creating temp for the value of expression like:
                //
                // `{ side_effects(); { let l = stuff(); the_value } }`
                //
                // it is usually better to focus on `the_value` rather than the entirety of block(s) surrounding it.
                let tail_of_block = if let ExprKind::Block { block: b } = &expr.kind { thir.thir[*b].expr } else { None };
                let adjusted_span = if let Some(tail_ex) = tail_of_block {
                    let mut expr = &thir.thir[tail_ex];
                    loop {
                        match &expr.kind {
                            ExprKind::Block { block: b } if thir.thir[*b].expr.is_some() => {
                                expr = &thir.thir[thir.thir[*b].expr.unwrap()];
                            }
                            ExprKind::Scope { value: nested_expr, .. } => {
                                expr = &thir.thir[*nested_expr];
                            }
                            _ => break,
                        }
                    }
                    self.block_context.push(BlockFrame::TailExpr { info: BlockTailInfo { tail_result_is_ignored: true, span: expr.span } });
                    Some(expr.span)
                } else {
                    None
                };

                let temp = unpack!(
                    block = self.as_temp(block, TempLifetime { temp_lifetime: statement_scope, backwards_incompatible: None }, expr_id, Mutability::Not)
                );

                if let Some(span) = adjusted_span {
                    self.local_decls[temp.0 as usize].source_info.span = span;
                    self.block_context.pop();
                }

                Ok(block.unit())
            }
        }
    }
}
