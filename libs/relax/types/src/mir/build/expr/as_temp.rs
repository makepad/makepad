//! rustc_mir_build/src/builder/expr/as_temp.rs. Owner: lane B.
use crate::mir::build::builder::{Builder, TempLifetime};
use crate::mir::build::cfg::{BlockAnd, BlockAndExtension};
use crate::mir::build::scope::DropKind;
use crate::mir::build::thir::{ExprId, ExprKind};
use crate::mir::build::BuildResult;
use crate::mir::{BasicBlock, Local, LocalInfo, LocalKind, Mutability, Place, Statement, StatementKind};
use crate::region_scope::ScopeData;

impl Builder<'_> {
    /// Compile `expr` into a fresh temporary. This is used when building up rvalues so as to freeze the value that
    /// will be consumed.
    pub fn as_temp(&mut self, block: BasicBlock, temp_lifetime: TempLifetime, expr_id: ExprId, mutability: Mutability) -> BuildResult<BlockAnd<Local>> {
        self.as_temp_inner(block, temp_lifetime, expr_id, mutability)
    }

    fn as_temp_inner(&mut self, mut block: BasicBlock, temp_lifetime: TempLifetime, expr_id: ExprId, mutability: Mutability) -> BuildResult<BlockAnd<Local>> {
        let thir = self.thir;
        let expr = &thir.thir[expr_id];
        let expr_span = expr.span;
        let source_info = self.source_info(expr_span);
        if let ExprKind::Scope { region_scope, value, .. } = expr.kind {
            return self.in_scope((region_scope, source_info), |this| this.as_temp(block, temp_lifetime, value, mutability));
        }

        let expr_ty = expr.ty;
        let deduplicate_temps = self.fixed_temps_scope.is_some() && self.fixed_temps_scope == temp_lifetime.temp_lifetime;
        let temp = if let Some(temp_index) = deduplicate_temps.then(|| self.fixed_temps.get(&expr_id)).flatten() {
            *temp_index
        } else {
            let local_info = match &expr.kind {
                ExprKind::StaticRef { def, .. } => LocalInfo::StaticRef { def_id: *def, is_thread_local: false },
                ExprKind::ThreadLocalRef(def) => LocalInfo::StaticRef { def_id: *def, is_thread_local: true },
                ExprKind::NamedConst { def, .. } | ExprKind::ConstParam { def, .. } => LocalInfo::ConstRef { def_id: *def },
                _ if self.currently_in_block_tail().is_some() => LocalInfo::BlockTailTemp(self.currently_in_block_tail().unwrap()),
                // Edition 2024 `if` condition temporaries are subject to the rescope rules.
                _ if temp_lifetime.temp_lifetime.is_some_and(|scope| self.root.scope_tree.scopes[scope.0 as usize].data == ScopeData::IfThenRescope) => {
                    match self.root.scope_tree.scopes[temp_lifetime.temp_lifetime.unwrap().0 as usize].node {
                        crate::region_scope::Node::Expr(e) => LocalInfo::IfThenRescopeTemp { if_then: e },
                        _ => LocalInfo::Boring,
                    }
                }
                _ => LocalInfo::Boring,
            };
            self.new_local(expr_ty, if mutability == Mutability::Not { Mutability::Not } else { Mutability::Mut }, LocalKind::Temp, local_info, expr_span)
        };
        if deduplicate_temps {
            self.fixed_temps.insert(expr_id, temp);
        }
        let temp_place = Place::local(temp);

        match &expr.kind {
            // Don't bother with StorageLive and Dead for these temporaries, they are never assigned.
            ExprKind::Break { .. } | ExprKind::Continue { .. } | ExprKind::Return { .. } => (),
            ExprKind::Block { block: b }
                if thir.thir[*b].expr.is_none()
                    && !thir.thir[*b].targeted_by_break
                    && self.root.cx.tcx.tys.kind(expr_ty) == &crate::types::TyKind::Never => {}
            _ => {
                self.cfg.push(block, Statement { source_info, kind: StatementKind::StorageLive(temp) });
                // In constants, `temp_lifetime` is `None` for temporaries that live for the `'static` lifetime.
                // Thus we do not drop them.
                if let Some(temp_lifetime) = temp_lifetime.temp_lifetime {
                    self.schedule_drop(expr_span, temp_lifetime, temp, DropKind::Storage)?;
                }
            }
        }

        block = self.expr_into_dest(temp_place, block, expr_id)?.into_block();

        if let Some(temp_lifetime) = temp_lifetime.temp_lifetime {
            self.schedule_drop(expr_span, temp_lifetime, temp, DropKind::Value)?;
        }

        if let Some(backwards_incompatible) = temp_lifetime.backwards_incompatible {
            self.schedule_backwards_incompatible_drop(expr_span, backwards_incompatible, temp)?;
        }

        Ok(block.and(temp))
    }
}
