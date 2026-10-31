//! rustc_mir_build/src/builder/expr/as_operand.rs: compile an expression into an operand.
use super::category::Category;
use super::super::builder::{Builder, TempLifetime};
use super::super::cfg::{BlockAnd, BlockAndExtension};
use super::super::thir::{ExprId, ExprKind};
use super::super::BuildResult;
use crate::mir::{BasicBlock, LocalInfo, Mutability, Operand, Place};

impl Builder<'_> {
    /// A temporary lifetime restricted to just the local scope.
    pub fn local_temp_lifetime(&self) -> TempLifetime {
        TempLifetime { temp_lifetime: Some(self.local_scope()), backwards_incompatible: None }
    }

    /// An operand suitable for use until the end of the current scope expression. It is *not valid*
    /// after the enclosing `ExprKind::Scope` has ended: never return it from functions.
    pub fn as_local_operand(&mut self, block: BasicBlock, expr: ExprId) -> BuildResult<BlockAnd<Operand>> {
        let scope = self.local_temp_lifetime();
        self.as_operand(block, scope, expr, LocalInfo::Boring, true)
    }

    /// As `as_local_operand`, suitable also as a call argument (see `as_call_operand`).
    pub fn as_local_call_operand(&mut self, block: BasicBlock, expr: ExprId) -> BuildResult<BlockAnd<Operand>> {
        let scope = self.local_temp_lifetime();
        self.as_call_operand(block, scope, expr)
    }

    /// Compile `expr` into a value that can be used as an operand. If `expr` is a place like `x`,
    /// this introduces a temporary `tmp = x`, so that the value of `x` is captured at this time.
    ///
    /// If a temporary is needed, `local_info` is its `LocalInfo` unless `as_temp` already gave it a
    /// more specific one. `needs_temporary`: rustc `NeedsTemporary::Maybe` (true) or `No` (false:
    /// the operand is consumed at once, so a constant never needs a temporary).
    /// The operand is known to be live until the end of `scope`.
    pub fn as_operand(&mut self, mut block: BasicBlock, scope: TempLifetime, expr_id: ExprId, local_info: LocalInfo, needs_temporary: bool) -> BuildResult<BlockAnd<Operand>> {
        let expr = &self.thir.thir[expr_id];
        if let ExprKind::Scope { region_scope, value, .. } = expr.kind {
            let source_info = self.source_info(expr.span);
            return self.in_scope((region_scope, source_info), |this| this.as_operand(block, scope, value, local_info, needs_temporary));
        }

        let category = Category::of(&expr.kind).expect("a non-scope expression has a category");
        let (ty, env) = (expr.ty, self.root.cx.env);
        match category {
            Category::Constant if !needs_temporary || !self.root.cx.tcx.needs_drop(self.root.cx.prog, env, ty) => {
                let constant = self.as_constant(expr_id)?;
                Ok(block.and(Operand::Constant(Box::new(constant))))
            }
            Category::Constant | Category::Place | Category::Rvalue(..) => {
                let BlockAnd(b, operand) = self.as_temp(block, scope, expr_id, Mutability::Mut)?;
                block = b;
                // Overwrite temp local info if we have something more interesting to record.
                if !matches!(local_info, LocalInfo::Boring) {
                    let decl_info = &mut self.local_decls[operand.0 as usize].local_info;
                    if let LocalInfo::Boring | LocalInfo::BlockTailTemp(_) = decl_info {
                        *decl_info = local_info;
                    }
                }
                Ok(block.and(Operand::Move(Place::local(operand))))
            }
        }
    }

    /// An operand suitable as a call argument. Almost always `as_operand`; the unsized-by-value
    /// parameter case of rustc (`#![feature(unsized_fn_params)]`, moving `*box`) is a feature this
    /// checker does not build.
    pub fn as_call_operand(&mut self, block: BasicBlock, scope: TempLifetime, expr_id: ExprId) -> BuildResult<BlockAnd<Operand>> {
        let expr = &self.thir.thir[expr_id];
        if let ExprKind::Scope { region_scope, value, .. } = expr.kind {
            let source_info = self.source_info(expr.span);
            return self.in_scope((region_scope, source_info), |this| this.as_call_operand(block, scope, value));
        }
        self.as_operand(block, scope, expr_id, LocalInfo::Boring, true)
    }
}
