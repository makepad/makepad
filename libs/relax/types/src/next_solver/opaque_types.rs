// ported from N:rustc_next_trait_solver/src/solve/project_goals/opaque_types.rs @8d1a764
// ported from N:rustc_next_trait_solver/src/solve/assembly/mod.rs @8d1a764
use super::*;
use super::eval_ctxt::EvalCtxt;
use super::assembly::*;
use crate::infer::opaque_types::{OpaqueTypeKey, ProvisionalHiddenType};
use crate::tcx::TypingMode;
use crate::types::{TyKind, Types, AliasTyKind, RegionKind};
use crate::types::fold::{TypeFolder, super_fold_ty};
impl EvalCtxt<'_, '_> {
    pub(super) fn normalize_opaque_type(&mut self, env: &ParamEnvId, p: &ProjectionClause) -> QueryResultOrRerunNonErased {
        let tcx = self.delegate.tcx; let (def, args) = self.alias_args(&p.projection_term);
        let Term::Ty(expected) = p.term else { unreachable!() };
        let owners = match self.delegate.typing_mode.get() {
            TypingMode::Coherence => {
                self.add_item_bounds_for_hidden_type(env, def, &args, expected)?;
                self.add_goal(GoalSource::Misc, Goal { param_env: env.clone(), predicate: Predicate::dummy(PredicateKind::Ambiguous) })?;
                return self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes);
            }
            TypingMode::Typeck { defining_opaque_types_and_generators } => Some(defining_opaque_types_and_generators),
            TypingMode::PostTypeckUntilBorrowck { defining_opaque_types } => Some(defining_opaque_types),
            TypingMode::PostBorrowck { defined_opaque_types } => Some(defined_opaque_types),
            TypingMode::Reflection | TypingMode::PostAnalysis | TypingMode::Codegen => None,
            TypingMode::ErasedNotCoherence => return Err(self.opaque_accesses.rerun_always()),
        };
        if owners.is_some_and(|o| !tcx.defining_opaques(o).contains(&def)) {
            self.eq(env, self.rigid_alias_term(&p.projection_term), p.term)?;
        } else if matches!(self.delegate.typing_mode.get(), TypingMode::Typeck { .. } | TypingMode::PostTypeckUntilBorrowck { .. }) {
            let mut normalized_args = Vec::new();
            for a in &args { normalized_args.push(match *a {
                GenericArg::Lifetime(r) => GenericArg::Lifetime(r),
                GenericArg::Type(t) => { let Term::Ty(t) = self.structurally_normalize_term(env, Term::Ty(t))? else { unreachable!() }; GenericArg::Type(t) }
                GenericArg::Const(c) => { let Term::Const(c) = self.structurally_normalize_term(env, Term::Const(c))? else { unreachable!() }; GenericArg::Const(c) }
            }); }
            let key = OpaqueTypeKey { def_id: def, args: normalized_args.clone().into() };
            let previous = self.delegate.inner.borrow_mut().opaque_types().register(key, ProvisionalHiddenType::new(expected));
            if let Some(prev) = previous { self.eq(env, p.term, Term::Ty(prev))?; }
            else if matches!(self.delegate.typing_mode.get(), TypingMode::PostTypeckUntilBorrowck { .. }) {
                let actual = self.type_of(def, &args)?;
                let actual = FreshErasedRegions(self.delegate).fold_ty(&tcx.tys, actual, 0);
                let actual = self.normalize_ty(env, actual)?;
                self.eq(env, p.term, Term::Ty(actual))?;
            }
            self.add_item_bounds_for_hidden_type(env, def, &normalized_args, expected)?;
        } else {
            let mut actual = self.type_of(def, &args)?;
            if matches!(self.delegate.typing_mode.get(), TypingMode::PostBorrowck { .. }) { actual = FreshErasedRegions(self.delegate).fold_ty(&tcx.tys, actual, 0); }
            let actual = self.normalize_ty(env, actual)?; self.eq(env, p.term, Term::Ty(actual))?;
        }
        self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
    }
    fn add_item_bounds_for_hidden_type(&mut self, env: &ParamEnvId, def: DefId, args: &[GenericArg], expected: TyId) -> Result<(), NoSolutionOrRerunNonErased> {
        let tcx = self.delegate.tcx;
        let alias = tcx.alias_ty_with_args(AliasTyKind::Opaque { def_id: def }, args);
        for p in tcx.item_bounds(self.solver.prog, def) {
            let p = tcx.instantiate_predicate(self.solver.prog, def, args, &p);
            let p = fold::fold_predicate(&tcx.tys, &p, &mut ReplaceOpaque { alias, expected });
            self.add_goal(GoalSource::Misc, Goal { param_env: env.clone(), predicate: p })?;
        }
        Ok(())
    }
    pub(super) fn try_assemble_bounds_via_registered_opaques(&mut self, env: &ParamEnvId, goal: &GoalKind, from: AssembleCandidatesFrom, candidates: &mut Vec<Candidate>) -> Result<(), NoSolutionOrRerunNonErased> {
        let self_ty = goal.self_ty(self); let tcx = self.delegate.tcx;
        let TyKind::Infer(var) = tcx.tys.kind(self_ty) else { unreachable!() };
        let mut opaque_types = Vec::new();
        if matches!(self.delegate.typing_mode.get(), TypingMode::Typeck { .. }) {
            let values = self.delegate.inner.borrow_mut().opaque_types().iter_lookup_table().to_vec();
            for (key, hidden) in values {
                if let TyKind::Infer(other) = tcx.tys.kind(self.delegate.shallow_resolve(hidden.ty)) {
                    let mut inner = self.delegate.inner.borrow_mut(); let mut table = inner.type_variables();
                    if table.sub_unification_table_root_var(*var) == table.sub_unification_table_root_var(*other) { opaque_types.push(key); }
                }
            }
        }
        if matches!(self.delegate.typing_mode.get(), TypingMode::ErasedNotCoherence) {
            self.opaque_accesses.update(AccessedOpaques { rerun: typing_mode::RerunCondition::AnyOpaqueHasInferAsHidden })?;
        }
        if opaque_types.is_empty() {
            candidates.push(self.forced_ambiguity(MaybeInfo::AMBIGUOUS)?);
            return Ok(());
        }
        {
            for key in opaque_types {
                let alias = tcx.alias_ty_with_args(AliasTyKind::Opaque { def_id: key.def_id }, &key.args);
                for p in tcx.item_bounds(self.solver.prog, key.def_id) {
                    let p = tcx.instantiate_predicate(self.solver.prog, key.def_id, &key.args, &p);
                    let subject = match &p.kind {
                        PredicateKind::Clause(ClauseKind::Trait(t)) => t.trait_ref.args.first().copied(),
                        PredicateKind::Clause(ClauseKind::Projection(p)) => self.alias_trait_ref(&p.projection_term).args.first().copied(),
                        _ => None,
                    };
                    if !matches!(subject, Some(GenericArg::Type(t)) if matches!((tcx.tys.kind(t), tcx.tys.kind(alias)), (TyKind::Alias(_, a), TyKind::Alias(_, b)) if a == b)) { continue; }
                    let p = fold::fold_predicate(&tcx.tys, &p, &mut ReplaceOpaque { alias, expected: self_ty });
                    if !self.assumption_matches_kind(goal, &p) { continue; }
                    let (result, head_usages) = self.probe().enter_single_candidate(CandidateSource::AliasBound(true), |ecx| {
                        ecx.match_assumption(env, goal, &p)?;
                        ecx.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS)
                    });
                    if let Ok(result) = map_err_to_rerun(result)? { candidates.push(Candidate { source: CandidateSource::AliasBound(true), result, head_usages }); }
                }
            }
            if matches!(from, AssembleCandidatesFrom::All) {
                // assembly/mod.rs:1210-1245: only blanket impls, with forced ambiguity.
                if let Some(impls) = tcx.trait_impls.get(&(crate::tcx::TyKey::Other, goal.trait_ref(self).def_id)) {
                    for &imp in impls {
                        if !matches!(tcx.tys.kind(tcx.impl_self[&imp]), TyKind::Param(_)) { continue; }
                        if let Some(candidate) = self.consider_impl_candidate(env, goal, imp, |ecx| {
                            if !matches!(tcx.tys.kind(ecx.delegate.shallow_resolve(self_ty)), TyKind::Infer(_)) { return Err(NoSolution.into()); }
                            ecx.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS)
                        })? { candidates.push(candidate); }
                    }
                }
            }
        }
        if candidates.is_empty() {
            candidates.push(self.forced_ambiguity(MaybeInfo {
                opaque_types_jank: OpaqueTypesJank::ErrorIfRigidSelfTy,
                ..MaybeInfo::AMBIGUOUS
            })?);
        }
        Ok(())
    }
}
struct ReplaceOpaque { alias: TyId, expected: TyId }
impl TypeFolder for ReplaceOpaque {
    fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId { if t == self.alias { tys.shift_in(self.expected, depth) } else { super_fold_ty(tys, t, self, depth) } }
}
struct FreshErasedRegions<'a, 'tcx>(&'a crate::infer::InferCtxt<'tcx>);
impl TypeFolder for FreshErasedRegions<'_, '_> {
    fn fold_region(&mut self, tys: &Types, r: RegionId, _: u32) -> RegionId { if tys.region_kind(r) == RegionKind::ReErased { self.0.next_region_var() } else { r } }
}
