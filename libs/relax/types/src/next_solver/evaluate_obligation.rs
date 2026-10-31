// ported from N:rustc_trait_selection/src/traits/query/evaluate_obligation.rs @8d1a764
// ported from N:rustc_middle/src/traits/select.rs @8d1a764
use super::*;
use crate::infer::InferCtxt;
use crate::program::Program;
use crate::tcx::{Tcx, TypingEnv};
use super::fulfill::{ObligationCtxt, NextSolverError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvaluationResult {
    EvaluatedToOk,
    EvaluatedToOkModuloRegions,
    EvaluatedToOkModuloOpaqueTypes,
    EvaluatedToAmbig,
    EvaluatedToErr,
}
impl EvaluationResult {
    pub fn must_apply_modulo_regions(self) -> bool { self <= Self::EvaluatedToOkModuloRegions }
    pub fn may_apply(self) -> bool { self != Self::EvaluatedToErr }
}

impl InferCtxt<'_> {
    /// evaluate_obligation.rs:95-114: property evaluation rolls back every
    /// inference effect, and records why a success is conditional.
    pub fn evaluate_obligation(&self, prog: &Program, goal: Goal) -> EvaluationResult {
        use EvaluationResult::*;
        self.probe(|snapshot| {
            let mut ocx = ObligationCtxt::new(self, prog);
            ocx.register_obligation(goal);
            let mut result = EvaluatedToOk;
            for error in ocx.engine.evaluate_obligations_error_on_ambiguity(self, prog) {
                match error {
                    NextSolverError::TrueError(_) => return EvaluatedToErr,
                    _ => result = result.max(EvaluatedToAmbig),
                }
            }
            if self.opaque_types_added_in_snapshot(snapshot) { result = result.max(EvaluatedToOkModuloOpaqueTypes); }
            else if self.region_constraints_added_in_snapshot(snapshot) { result = result.max(EvaluatedToOkModuloRegions); }
            result
        })
    }

    /// rustc_trait_selection/src/infer.rs:89-107.
    pub fn type_implements_trait(&self, prog: &Program, param_env: ParamEnvId, trait_ref: TraitRef) -> EvaluationResult {
        self.evaluate_obligation(prog, Goal { param_env, predicate: collect::trait_predicate(trait_ref.def_id, trait_ref.args) })
    }
}

impl Tcx {
    /// A property query owns no body inference variables: the typing environment's
    /// owner builds its own (rustc_ty_utils/src/common_traits.rs:47-57); free regions
    /// are erased.
    pub fn type_implements_trait(&self, prog: &Program, typing_env: TypingEnv, mut trait_ref: TraitRef) -> EvaluationResult {
        for arg in &mut trait_ref.args {
            *arg = match *arg {
                GenericArg::Type(t) => GenericArg::Type(self.tys.erase_regions(t)),
                GenericArg::Const(c) => GenericArg::Const(self.tys.expect_const(self.tys.erase_regions(self.tys.intern(crate::types::TyKind::Const(c))))),
                GenericArg::Lifetime(_) => GenericArg::Lifetime(RegionId::ERASED),
            };
        }
        let infcx = InferCtxt::new(self, typing_env.mode, typing_env.limit);

        infcx.type_implements_trait(prog, typing_env.param_env, trait_ref)
    }

    /// rustc_ty_utils/src/common_traits.rs:11-13,47-57 (is_copy_raw / is_trait_raw).
    pub fn is_copy_modulo_regions(&self, prog: &Program, typing_env: TypingEnv, ty: TyId) -> bool {
        let Some(copy) = self.lang_items.get(crate::lang_items::LangItem::Copy) else { return false };
        self.type_implements_trait(prog, typing_env, TraitRef { def_id: copy, args: vec![GenericArg::Type(ty)] }).must_apply_modulo_regions()
    }
}
