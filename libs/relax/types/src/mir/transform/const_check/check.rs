//! The `Visitor` responsible for actually checking a `mir::Body` for invalid operations:
//! rustc_const_eval/src/check_consts/check.rs (1.97.1), without stability, intrinsic stability and
//! coroutine checks (see the module docs of `const_check`).
use super::ops::{self, ConstError, Importance};
use super::{ConstContext, ConstCx};
use crate::mir::dataflow::{iterate_to_fixpoint, BitSet, MaybeStorageLive};
use crate::mir::visit::Visitor;
use crate::mir::*;
use crate::program::DefKind;
use crate::types::{TyId, TyKind};
use super::super::qualifs::{in_any_value_of_ty, in_place, ConstQualifs, HasMutInterior, NeedsDrop, NeedsNonConstDrop, Qualif, QualifResults};

/// The three flow-sensitive qualif analyses, each computed on first use.
#[derive(Default)]
struct Qualifs {
    has_mut_interior: Option<QualifResults<HasMutInterior>>,
    needs_drop: Option<QualifResults<NeedsDrop>>,
    needs_non_const_drop: Option<QualifResults<NeedsNonConstDrop>>,
}

impl Qualifs {
    /// Returns `true` if `local` is `NeedsDrop` at the given `Location`.
    ///
    /// Only updates the cursor if absolutely necessary
    fn needs_drop(&mut self, ccx: &ConstCx<'_>, local: Local, location: Location) -> bool {
        let ty = ccx.body.local_decls[local.0 as usize].ty;
        if !NeedsDrop::in_any_value_of_ty(ccx, ty) {
            return false;
        }
        self.needs_drop.get_or_insert_with(|| QualifResults::compute(ccx)).contains_before(ccx, local, location)
    }

    /// Returns `true` if `local` is `NeedsNonConstDrop` at the given `Location`.
    fn needs_non_const_drop(&mut self, ccx: &ConstCx<'_>, local: Local, location: Location) -> bool {
        let ty = ccx.body.local_decls[local.0 as usize].ty;
        if !NeedsNonConstDrop::in_any_value_of_ty(ccx, ty) {
            return false;
        }
        self.needs_non_const_drop.get_or_insert_with(|| QualifResults::compute(ccx)).contains_before(ccx, local, location)
    }

    /// Returns `true` if `local` is `HasMutInterior` at the given `Location`.
    fn has_mut_interior(&mut self, ccx: &ConstCx<'_>, local: Local, location: Location) -> bool {
        let ty = ccx.body.local_decls[local.0 as usize].ty;
        if !HasMutInterior::in_any_value_of_ty(ccx, ty) {
            return false;
        }
        self.has_mut_interior.get_or_insert_with(|| QualifResults::compute(ccx)).contains_before(ccx, local, location)
    }

    fn in_return_place(&mut self, ccx: &ConstCx<'_>, tainted_by_errors: bool) -> ConstQualifs {
        // Find the `Return` terminator if one exists.
        //
        // If no `Return` terminator exists, this MIR is divergent. Just return the conservative
        // qualifs for the return type.
        let return_block = ccx.body.basic_blocks.iter().position(|block| matches!(block.terminator.as_ref().map(|t| &t.kind), Some(TerminatorKind::Return)));

        let Some(return_block) = return_block else {
            return in_any_value_of_ty(ccx, ccx.body.local_decls[0].ty, tainted_by_errors);
        };

        let return_loc = Location { block: BasicBlock(return_block as u32), statement_index: ccx.body.basic_blocks[return_block].statements.len() };

        ConstQualifs {
            needs_drop: self.needs_drop(ccx, Local(0), return_loc),
            needs_non_const_drop: self.needs_non_const_drop(ccx, Local(0), return_loc),
            has_mut_interior: self.has_mut_interior(ccx, Local(0), return_loc),
            tainted_by_errors,
        }
    }
}

pub struct Checker<'a> {
    ccx: &'a ConstCx<'a>,
    qualifs: Qualifs,

    /// The span of the current statement.
    span: Span,

    /// A set that stores for each local whether it is "transient", i.e. guaranteed to be dead when
    /// this MIR body returns.
    transient_locals: Option<BitSet>,

    error_emitted: bool,
    errors: Vec<ConstError>,
    secondary_errors: Vec<ConstError>,
}

impl<'a> Checker<'a> {
    pub fn new(ccx: &'a ConstCx<'a>) -> Self {
        Checker { span: ccx.body.span, ccx, qualifs: Default::default(), transient_locals: None, error_emitted: false, errors: Vec::new(), secondary_errors: Vec::new() }
    }

    pub fn check_body(&mut self) {
        let body = self.ccx.body;

        // `async` functions cannot be `const fn`. This is checked during AST lowering, so there's no
        // need to emit duplicate errors here.
        if body.coroutine.is_some() {
            return;
        }

        self.visit_body_blocks(body);

        // If we got through const-checking without emitting any "primary" errors, emit any
        // "secondary" errors if they occurred. Otherwise, cancel the "secondary" errors.
        let secondary_errors = std::mem::take(&mut self.secondary_errors);
        if !self.error_emitted {
            self.errors.extend(secondary_errors);
            self.error_emitted = !self.errors.is_empty();
        }
    }

    /// The errors found, in block order.
    pub fn finish(self) -> Vec<ConstError> {
        self.errors
    }

    pub fn qualifs_in_return_place(&mut self) -> ConstQualifs {
        let tainted = self.error_emitted;
        self.qualifs.in_return_place(self.ccx, tainted)
    }

    /// Blocks in index order (rustc `visit_body`), cleanup blocks skipped.
    fn visit_body_blocks(&mut self, body: &Body) {
        for (bb, block) in body.basic_blocks.iter().enumerate() {
            let bb = BasicBlock(bb as u32);
            // We don't const-check basic blocks on the cleanup path since we never unwind during
            // const-eval: a panic causes an immediate compile error. In other words, cleanup blocks
            // are unreachable during const-eval.
            //
            // We can't be more conservative (e.g., by const-checking cleanup blocks anyways) because
            // locals that would never be dropped during normal execution are sometimes dropped
            // during unwinding, which means backwards-incompatible live-drop errors.
            if block.is_cleanup {
                continue;
            }
            self.visit_basic_block_data(bb, block);
        }
    }

    fn local_is_transient(&mut self, local: Local) -> bool {
        let ccx = self.ccx;
        self.transient_locals
            .get_or_insert_with(|| {
                // A local is "transient" if it is guaranteed dead at all `Return`. So first compute
                // the set of "maybe live" locals at each program point.
                let body = ccx.body;
                let analysis = MaybeStorageLive::new(body);
                let results = iterate_to_fixpoint(body, &analysis);
                let mut cursor = results.cursor();

                // And then check all `Return` in the MIR, and if a local is "maybe live" at a
                // `Return` then it is definitely not transient.
                let mut transient = BitSet::new_filled(body.local_decls.len());
                // Make sure to only visit reachable blocks, the dataflow engine can ICE otherwise.
                for bb in super::super::simplify::preorder(body) {
                    let data = &body.basic_blocks[bb.0 as usize];
                    if matches!(data.terminator.as_ref().map(|t| &t.kind), Some(TerminatorKind::Return)) {
                        cursor.seek_after_primary_effect(Location { block: bb, statement_index: data.statements.len() });
                        // If a local may be live here, it is definitely not transient.
                        transient.subtract(cursor.get());
                    }
                }

                transient
            })
            .contains(local.0 as usize)
    }

    /// Emits an error if an expression cannot be evaluated in the current context.
    fn check_op(&mut self, op: ConstError) {
        match op.importance {
            Importance::Primary => {
                self.error_emitted = true;
                self.errors.push(op);
            }
            Importance::Secondary => self.secondary_errors.push(op),
        }
    }

    /// Returns whether this place can possibly escape the evaluation of the current const/static
    /// initializer. The check assumes that all already existing pointers and references point to
    /// non-escaping places.
    fn place_may_escape(&mut self, place: &Place) -> bool {
        let is_transient = match self.ccx.const_kind() {
            // In a const fn all borrows are transient or point to the places given via references in
            // the arguments (so we already checked them with TransientMutBorrow/MutBorrow as
            // appropriate). The borrow checker guarantees that no new non-transient borrows are
            // created.
            ConstContext::ConstFn => true,
            _ => {
                // For indirect places, we are not creating a new permanent borrow, it's just as
                // transient as the already existing one. Locals with StorageDead do not live beyond
                // the evaluation and can thus safely be borrowed without being able to be leaked to
                // the final value of the constant.
                // Note: This is only sound if every local that has a `StorageDead` has a
                // `StorageDead` in every control flow path leading to a `return` terminator.
                super::super::qualifs::is_indirect(place) || self.local_is_transient(place.local)
            }
        };
        // Transient places cannot possibly escape because the place doesn't exist any more at the
        // end of evaluation.
        !is_transient
    }

    fn check_drop_terminator(&mut self, dropped_place: &Place, location: Location, terminator_span: Span) {
        let ccx = self.ccx;
        let ty_of_dropped_place = super::super::qualifs::place_ty(ccx, dropped_place);

        let as_local = if dropped_place.projection.is_empty() { Some(dropped_place.local) } else { None };
        let needs_drop = match as_local {
            Some(local) => self.qualifs.needs_drop(ccx, local, location),
            None => NeedsDrop::in_any_value_of_ty(ccx, ty_of_dropped_place),
        };
        // If this type doesn't need a drop at all, then there's nothing to enforce.
        if !needs_drop {
            return;
        }

        let mut err_span = self.span;
        let needs_non_const_drop = match as_local {
            Some(local) => {
                // Use the span where the local was declared as the span of the drop error.
                err_span = ccx.body.local_decls[local.0 as usize].source_info.span;
                self.qualifs.needs_non_const_drop(ccx, local, location)
            }
            None => NeedsNonConstDrop::in_any_value_of_ty(ccx, ty_of_dropped_place),
        };

        // `LiveDrop`: forbidden when the value needs a non-const drop; with `const_destruct` (the
        // gate of the other case) a const drop is allowed.
        let _ = terminator_span;
        if needs_non_const_drop {
            self.check_op(ops::live_drop(ccx, ty_of_dropped_place, err_span));
        }
    }

    fn is_int_bool_float_or_char(&self, ty: TyId) -> bool {
        matches!(self.ccx.tcx.tys.kind(ty), TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_))
    }
}

impl Visitor for Checker<'_> {
    fn visit_rvalue(&mut self, rvalue: &Rvalue, location: Location) {
        self.super_rvalue(rvalue, location);
        let ccx = self.ccx;

        match rvalue {
            Rvalue::ThreadLocalRef(_) => self.check_op(ops::thread_local_access(self.span)),

            Rvalue::Use(..) | Rvalue::CopyForDeref(..) | Rvalue::Repeat(..) | Rvalue::Discriminant(..) => {}

            Rvalue::Aggregate(..) => {}

            Rvalue::Ref(_, BorrowKind::Mut { .. }, place) | Rvalue::RawPtr(RawPtrKind::Mut, place) => {
                // Inside mutable statics, we allow arbitrary mutable references. We've allowed
                // `static mut FOO = &mut [elements];` for a long time (the exact reasons why are lost
                // to history), and there is no reason to restrict that to arrays and slices.
                let is_allowed = ccx.const_kind() == ConstContext::Static(true);

                if !is_allowed && self.place_may_escape(place) {
                    self.check_op(ops::escaping_mut_borrow(ccx, self.span));
                }
            }

            Rvalue::Ref(_, BorrowKind::Shared | BorrowKind::Fake(_), place) | Rvalue::RawPtr(RawPtrKind::Const, place) => {
                let qualifs = &mut self.qualifs;
                let borrowed_place_has_mut_interior = in_place::<HasMutInterior, _>(ccx, &mut |local| qualifs.has_mut_interior(ccx, local, location), place);

                if borrowed_place_has_mut_interior && self.place_may_escape(place) {
                    self.check_op(ops::escaping_cell_borrow(ccx, self.span));
                }
            }

            Rvalue::RawPtr(RawPtrKind::FakeForPtrMetadata, _) => {
                // These are only inserted for slice length, so the place must already be indirect.
                // This implies we do not have to worry about whether the borrow escapes.
            }

            Rvalue::Cast(CastKind::PointerCoercion(..), _, _) => {
                // These are all okay; they only change the type, not the data.
            }

            Rvalue::Cast(CastKind::PointerExposeProvenance, _, _) => {
                self.check_op(ops::raw_ptr_to_int_cast(self.span));
            }
            Rvalue::Cast(CastKind::PointerWithExposedProvenance, _, _) => {
                // Since no pointer can ever get exposed (rejected above), this is easy to support.
            }

            Rvalue::Cast(_, _, _) => {}

            Rvalue::UnaryOp(..) => {}

            Rvalue::BinaryOp(_op, ops) => {
                let lhs_ty = super::super::qualifs::operand_ty(ccx, &ops.0);
                let rhs_ty = super::super::qualifs::operand_ty(ccx, &ops.1);

                if self.is_int_bool_float_or_char(lhs_ty) && self.is_int_bool_float_or_char(rhs_ty) {
                    // Int, bool, float, and char operations are fine.
                } else if super::ops::is_ptr_like(ccx, lhs_ty) {
                    self.check_op(ops::raw_ptr_comparison(self.span));
                }
            }
        }
    }

    fn visit_source_info(&mut self, source_info: &SourceInfo) {
        self.span = source_info.span;
    }

    fn visit_statement(&mut self, statement: &Statement, location: Location) {
        self.visit_source_info(&statement.source_info);
        self.super_statement(statement, location);
    }

    fn visit_terminator(&mut self, terminator: &Terminator, location: Location) {
        self.visit_source_info(&terminator.source_info);
        self.super_terminator(terminator, location);
        let ccx = self.ccx;

        match &terminator.kind {
            TerminatorKind::Call { func, fn_span, call_source, args, .. } => {
                let fn_ty = super::super::qualifs::operand_ty(ccx, func);
                let fn_span = if fn_span.lo == 0 && fn_span.hi == 0 { self.span } else { *fn_span };

                let callee = match *ccx.tcx.tys.kind(fn_ty) {
                    TyKind::FnDef(def_id, ..) => def_id,
                    TyKind::FnPtr(..) => {
                        self.check_op(ops::fn_call_indirect(ccx, fn_span));
                        return;
                    }
                    _ => return,
                };
                self.check_call(callee, args, fn_span, *call_source);
            }

            // Forbid all `Drop` terminators unless the place being dropped is a local with no
            // projections that cannot be `NeedsNonConstDrop`.
            TerminatorKind::Drop { place: dropped_place, .. } => {
                self.check_drop_terminator(dropped_place, location, terminator.source_info.span);
            }

            TerminatorKind::TailCall { .. }
            | TerminatorKind::InlineAsm(_)
            | TerminatorKind::Yield { .. }
            | TerminatorKind::CoroutineDrop
            | TerminatorKind::UnwindTerminate(_)
            | TerminatorKind::Assert { .. }
            | TerminatorKind::FalseEdge { .. }
            | TerminatorKind::FalseUnwind { .. }
            | TerminatorKind::Goto { .. }
            | TerminatorKind::UnwindResume
            | TerminatorKind::Return
            | TerminatorKind::SwitchInt { .. }
            | TerminatorKind::Unreachable => {}
        }
    }
}

impl Checker<'_> {
    /// A call whose callee is a known function (a `FnDef`).
    fn check_call(&mut self, callee: crate::program::DefId, args: &[Operand], span: Span, call_source: CallSource) {
        let ccx = self.ccx;
        let (prog, tcx) = (ccx.prog, ccx.tcx);
        // `begin_panic` and `panic_display` accept generic types other than str; const eval of them
        // assumes `&str` / `&&str`. Both calls are otherwise allowed.
        if let Some(item) = tcx.lang_items.by_def.get(&callee).copied() {
            let ref_str = |ty: crate::types::TyId| matches!(*tcx.tys.kind(ty), TyKind::Ref(_, inner, _) if matches!(tcx.tys.kind(inner), TyKind::Str));
            let arg_ty = args.first().map(|a| super::super::qualifs::operand_ty(ccx, a));
            match item {
                crate::lang_items::LangItem::BeginPanic => {
                    if !arg_ty.is_some_and(ref_str) {
                        self.check_op(ops::panic_non_str(span));
                    }
                    return;
                }
                crate::lang_items::LangItem::PanicDisplay => {
                    let ok = arg_ty.is_some_and(|t| matches!(*tcx.tys.kind(t), TyKind::Ref(_, inner, _) if ref_str(inner)));
                    if !ok {
                        self.check_op(ops::panic_non_str(span));
                    }
                    return;
                }
                _ => {}
            }
        }
        let def = prog.def(callee);
        if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn) {
            return;
        }
        // Attempting to call a trait method? We can't determine the actual callee (the underlying
        // impl of the trait) here, so we have to do different checks than usual: trait methods are
        // never const on stable, only conditionally const under `const_trait_impl`.
        let parent = def.parent;
        let through_trait = parent != crate::program::NO_DEF
            && match prog.def(parent).kind {
                DefKind::Trait => true,
                DefKind::Impl => tcx.impl_trait.get(&parent).is_some(),
                _ => false,
            };
        let non_const = if through_trait {
            !ccx.features.const_traits
        } else {
            def.kind == DefKind::ForeignFn || !tcx.fn_sig(prog, callee).map_or(true, |s| s.is_const)
        };
        if non_const {
            self.check_op(ops::fn_call_non_const(ccx, callee, span, call_source));
        }
    }
}
