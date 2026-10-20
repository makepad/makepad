// ported from N:rustc_hir_typeck/src/fn_ctxt/checks.rs @8d1a764
// ported from N:rustc_hir_typeck/src/fn_ctxt/_impl.rs @8d1a764
use super::*;
use crate::infer::relate::{self, TypeRelation, TypeError};
use crate::consts::{Const, ConstKind, GenericArg};
use crate::types::{RegionId, variance::V};

impl Fcx<'_> {
    /// rustc_hir_typeck/src/expr.rs:660-703: all fn-item value paths,
    /// including constructors and callees, require Sized inputs (unless the
    /// calling crate enables unsized_fn_params) and always Sized output.
    /// Open each signature component's binder separately: this check only
    /// needs sizedness and must not relate distinct late-bound lifetimes.
    pub(super) fn defer_fn_item_sized_obligations(&mut self, ty: TyId, lo: u32) {
        let TyKind::FnDef(def, args, regions) = self.tcx.tys.kind(ty).clone() else { return };
        let Some(sig) = self.tcx.fn_item_sig(self.prog, def).cloned() else { return };
        let args = self.tcx.complete_args(self.prog, def, self.tcx.tys.list(args), regions);
        let unsized_fn_params = self.prog.crates[self.prog.def(self.b.def).krate as usize].features(self.prog).has("unsized_fn_params");
        for i in 0..sig.params.len() + 1 {
            if i < sig.params.len() && unsized_fn_params { continue; }
            let ty = if i < sig.params.len() { sig.params[i] } else { sig.ret };
            let poly = self.tcx.tys.intern(TyKind::FnPtr(crate::types::List::EMPTY, ty, sig.header, sig.bound_regions));
            let poly = self.tcx.instantiate_ty(self.prog, def, &args, poly);
            let opened = self.infcx.instantiate_binder_with_infer(poly);
            let TyKind::FnPtr(_, ty, _, _) = self.tcx.tys.kind(opened) else { unreachable!() };
            self.require_type_is_sized_deferred(*ty, lo);
        }
    }

    /// rustc_hir_typeck/src/fn_ctxt/_impl.rs:495-503: do not create
    /// secondary obligations from a type already containing an error.
    pub(super) fn require_type_is_sized_deferred(&mut self, ty: TyId, lo: u32) {
        if !self.tcx.tys.has_error(ty) { self.deferred_sized_obligations.push((ty, lo)); }
    }

    // fn_ctxt/checks.rs:229-235: each call input must be well formed.
    pub(super) fn register_wf(&mut self, ty: TyId, lo: u32) {
        // (a crash guard of the checked crates: a path with a wrong argument count reaches the solver)
        if self.tcx.validates(self.prog, self.file) && !crate::wfcheck::obligations::arity_ok(self.tcx, self.prog, ty) { return; }
        self.register_goal(Goal { param_env: self.param_env, predicate: Predicate::dummy(
            PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(ty)))) }, lo);
    }

    /// _impl.rs:114-138: settle obligations only when unresolved variables remain.
    pub(super) fn deeply_resolve_ignoring_regions_with_obligations(&mut self, ty: TyId) -> TyId {
        if !self.tcx.tys.has_infer(ty) { return ty; }
        let ty = self.infcx.deeply_resolve_ignoring_regions(ty);
        if !self.tcx.tys.has_infer(ty) { return ty; }
        self.retry_goals(false);
        self.infcx.deeply_resolve_ignoring_regions(ty)
    }
    /// rustc_hir_typeck/src/fn_ctxt/checks.rs:255-291: expectations leave the
    /// probe through inference fudging; no rolled-back variable escapes.
    pub(super) fn expected_inputs_for_expected_output(&mut self, ret: TyId, expected: TyId, inputs: &[TyId]) -> Option<Vec<Option<TyId>>> {
        use crate::next_solver::fulfill::ObligationCtxt;
        let formal_inputs = inputs;
        let ret = self.deeply_resolve_ignoring_regions_with_obligations(ret);
        // the next solver's signature has its aliases normalized to inference variables (their
        // projection goals registered): an alias left in the expectation would mention variables
        // popped with the fudge scope (fudge-inference-with-aliases)
        let normalized: Vec<TyId> = inputs.iter().map(|&t| self.norm(t)).collect();
        let inputs = &normalized[..];
        let tuple = self.infcx.fudge_inference_if_ok(|| {
            let mut ocx = ObligationCtxt::new(&self.infcx, self.prog);
            let goals = self.infcx.at(self.prog, self.param_env).sub(ret, expected).map_err(|_| crate::next_solver::NoSolution)?;
            ocx.register_obligations(goals);
            for &ty in inputs { ocx.register_obligation(Goal { param_env: self.param_env, predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(ty)))) }); }
            if !ocx.engine.try_evaluate_obligations(&self.infcx, self.prog).is_empty() { return Err(crate::next_solver::NoSolution); }
            Ok(self.tcx.tys.intern(TyKind::Tuple(self.tcx.tys.mk_list(inputs))))
        }).ok()?;
        let TyKind::Tuple(inputs) = self.tcx.tys.kind(tuple) else { unreachable!() };
        let mut result = Vec::with_capacity(self.tcx.tys.list(*inputs).len());
        for ((&expected, &formal), &norm) in self.tcx.tys.list(*inputs).iter().zip(formal_inputs).zip(normalized.iter()) {
            result.push(Some(if same_type_modulo_vars(self.tcx, self.prog, expected, norm) { formal } else { expected }));
        }
        Some(result)
    }

}

/// checks.rs:3575-3679: fudging that only renames variables must preserve the
/// formal input, whose existing obligations constrain argument coercion.
fn same_type_modulo_vars(tcx: &Tcx, prog: &Program, a: TyId, b: TyId) -> bool {
    struct SameModuloVars<'a> { tcx: &'a Tcx, prog: &'a Program }
    impl<'a> TypeRelation<'a> for SameModuloVars<'a> {
        fn tcx(&self) -> &'a Tcx { self.tcx }
        fn prog(&self) -> &Program { self.prog }
        fn variance(&self) -> V { V::Inv }
        fn set_variance(&mut self, _: V) {}
        fn tys(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError> {
            if a == b { return Ok(a); }
            match (self.tcx.tys.kind(a), self.tcx.tys.kind(b)) {
                (TyKind::Infer(_), TyKind::Infer(_)) | (TyKind::IntVar(_), TyKind::IntVar(_)) | (TyKind::FloatVar(_), TyKind::FloatVar(_)) => Ok(a),
                (TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_), _) | (_, TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_)) => Err(TypeError::Mismatch),
                (TyKind::Error, _) | (_, TyKind::Error) => Ok(self.tcx.tys.error),
                _ => relate::structurally_relate_tys(self, a, b),
            }
        }
        fn regions(&mut self, a: RegionId, _: RegionId) -> Result<RegionId, TypeError> { Ok(a) }
        fn consts(&mut self, a: Const, b: Const) -> Result<Const, TypeError> {
            if a == b { return Ok(a); }
            match (self.tcx.tys.const_kind(a), self.tcx.tys.const_kind(b)) {
                (ConstKind::Infer(_), ConstKind::Infer(_)) => Ok(a),
                (ConstKind::Infer(_), _) | (_, ConstKind::Infer(_)) => Err(TypeError::Mismatch),
                _ => relate::structurally_relate_consts(self, a, b),
            }
        }
        fn binders(&mut self, a: TyId, b: TyId) -> Result<TyId, TypeError> { relate::relate_fn_sig(self, a, b) }
        fn binders_args(&mut self, a: &[GenericArg], b: &[GenericArg]) -> Result<Vec<GenericArg>, TypeError> { relate::relate_args(self, a, b) }
    }
    SameModuloVars { tcx, prog }.tys(a, b).is_ok()
}
