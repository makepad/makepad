// ported from N:rustc_next_trait_solver/src/solve/mod.rs @8d1a764
// ported from N:rustc_next_trait_solver/src/solve/eval_ctxt/mod.rs @8d1a764
use super::*;
use super::eval_ctxt::EvalCtxt;
use crate::types::{TyKind};
use crate::consts::{ConstKind, IsRigid};

impl EvalCtxt<'_, '_> {
    pub(super) fn compute_goal(&mut self, goal: Goal) -> QueryResultOrRerunNonErased {
        let env = goal.param_env;
        let kind = self.delegate.enter_forall_predicate_and_leak_universe(&goal.predicate);
        match kind {
            PredicateKind::Clause(ClauseKind::Trait(p)) => self.compute_trait_goal(env, p),
            PredicateKind::Clause(ClauseKind::Projection(p)) => self.compute_projection_goal(env, p),
            PredicateKind::NormalizesTo(p) => self.compute_normalizes_to_goal(env, p),
            PredicateKind::Subtype(p) => self.compute_subtype_goal(env, p.a, p.b),
            PredicateKind::Coerce(p) => self.compute_subtype_goal(env, p.a, p.b),
            PredicateKind::Clause(ClauseKind::RegionOutlives(a, b)) => {
                self.delegate.inner.borrow_mut().unwrap_region_constraints().make_subregion(&self.delegate.tcx.tys, crate::infer::region_constraints::SubregionOrigin::RelateRegionParamBound(self.delegate.origin_span.get()), b, a, true);
                self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
            }
            PredicateKind::Clause(ClauseKind::TypeOutlives(ty, region)) => {
                let ty = self.normalize_ty(&env, ty)?;
                let certainty = if self.delegate.tcx.tys.has_infer(ty) || super::normalize::has_non_rigid_aliases(&self.delegate.tcx.tys, ty) {
                    Certainty::AMBIGUOUS
                } else { self.delegate.register_ty_outlives(ty, region); Certainty::Yes };
                self.evaluate_added_goals_and_make_canonical_response(certainty)
            }
            PredicateKind::Clause(ClauseKind::ConstEvaluatable(ct)) => {
                let ct = self.delegate.shallow_resolve_const(ct);
                let certainty = match self.delegate.tcx.tys.const_kind(ct) {
                    ConstKind::Value(_) | ConstKind::Error | ConstKind::Placeholder(_) | ConstKind::Alias(IsRigid::Yes, _) => Certainty::Yes,
                    ConstKind::Infer(_) => Certainty::AMBIGUOUS,
                    ConstKind::Alias(IsRigid::No, alias) => if self.evaluate_const(&env, &alias)?.is_some() { Certainty::Yes } else { Certainty::AMBIGUOUS },
                    ConstKind::Param(_) | ConstKind::Bound(..) => panic!("noncanonical const"),
                };
                self.evaluate_added_goals_and_make_canonical_response(certainty)
            }
            PredicateKind::Clause(ClauseKind::ConstArgHasType(ct, ty)) => {
                let ct = self.structurally_normalize_term(&env, Term::Const(ct))?;
                let Term::Const(ct) = ct else { unreachable!() };
                let ct_ty = match self.delegate.tcx.tys.const_kind(ct) {
                    ConstKind::Infer(_) => return self.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS),
                    ConstKind::Error => return self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes),
                    ConstKind::Value(v) => v.ty,
                    ConstKind::Placeholder(_) => {
                        let mut found = None;
                        for p in &*self.delegate.tcx.param_env_by_id(env).clauses {
                            if let PredicateKind::Clause(ClauseKind::ConstArgHasType(other, ty)) = p.kind {
                                if other == ct { found = Some(ty); break; }
                            }
                        }
                        found.expect("const placeholder type clause")
                    }
                    ConstKind::Alias(IsRigid::Yes, alias) => self.const_type(&alias)?,
                    _ => panic!("noncanonical const argument"),
                };
                self.eq(&env, Term::Ty(ct_ty), Term::Ty(ty))?;
                self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
            }
            PredicateKind::Clause(ClauseKind::WellFormed(term)) => self.compute_well_formed_goal(env, term),
            PredicateKind::DynCompatible(def) => {
                self.check_dyn_compatible(def)?;
                self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
            }
            PredicateKind::Ambiguous => self.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS),
            PredicateKind::ConstEquate(..) => panic!("ConstEquate is not emitted by the next solver"),
        }
    }
    fn compute_subtype_goal(&mut self, env: ParamEnvId, a: TyId, b: TyId) -> QueryResultOrRerunNonErased {
        let a = self.delegate.shallow_resolve(a);
        let b = self.delegate.shallow_resolve(b);
        if let (TyKind::Infer(a), TyKind::Infer(b)) = (self.delegate.tcx.tys.kind(a), self.delegate.tcx.tys.kind(b)) {
            self.delegate.inner.borrow_mut().type_variables().sub_unify(*a, *b);
            self.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS)
        } else {
            let goals = self.delegate.at(self.solver.prog, env).sub(a, b).map_err(|_| NoSolution)?;
            self.add_relate_goals(goals)?;
            self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
        }
    }
    pub(super) fn next_term_infer_of_alias_kind(&mut self, alias: &AliasTerm) -> Term {
        let term = match alias { AliasTerm::Ty(_) => Term::Ty(self.delegate.next_ty_var()), AliasTerm::Const(_) => Term::Const(self.delegate.next_const_var()) };
        self.inspect.add_var_value(match term { Term::Ty(t) => GenericArg::Type(t), Term::Const(c) => GenericArg::Const(c) }); term
    }
    pub(super) fn shallow_resolve_term(&self, term: Term) -> Term {
        match term { Term::Ty(ty) => Term::Ty(self.delegate.shallow_resolve(ty)), Term::Const(ct) => Term::Const(self.delegate.shallow_resolve_const(ct)) }
    }
    pub(super) fn structurally_normalize_term(&mut self, env: &ParamEnvId, term: Term) -> Result<Term, NoSolutionOrRerunNonErased> {
        let term = self.shallow_resolve_term(term);
        let alias = match term {
            Term::Ty(ty) => match self.delegate.tcx.tys.kind(ty) { TyKind::Alias(IsRigid::No, _) => Some(AliasTerm::Ty(ty)), _ => None },
            Term::Const(ct) => match self.delegate.tcx.tys.const_kind(ct) { ConstKind::Alias(IsRigid::No, alias) => Some(AliasTerm::Const(alias)), _ => None },
        };
        let Some(alias) = alias else { return Ok(term); };
        let value = self.next_term_infer_of_alias_kind(&alias);
        self.add_goal(GoalSource::TypeRelating, Goal { param_env: env.clone(), predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: alias, term: value }))) })?;
        self.try_evaluate_added_goals()?;
        Ok(fold::fold_term(&self.delegate.tcx.tys, value, &mut crate::infer::resolve::DeepResolverIgnoringRegions::new(self.delegate), 0))
    }
    pub(super) fn const_type(&self, alias: &crate::consts::AliasConst) -> Result<TyId, NoSolution> {
        let tcx = self.delegate.tcx;
        let def = alias.kind.def_id();
        let ty = tcx.type_of(self.solver.prog, def)?;
        Ok(tcx.instantiate_ty(self.solver.prog, def, &alias.args, ty))
    }
    pub(super) fn evaluate_const(&mut self, env: &ParamEnvId, alias: &crate::consts::AliasConst) -> Result<Option<crate::consts::Const>, NoSolutionOrRerunNonErased> {
        use crate::types::fold::TypeFolder;
        let tcx = self.delegate.tcx;
        let mut f = crate::infer::resolve::DeepResolverIgnoringRegions::new(self.delegate);
        let ct = tcx.tys.intern_const(ConstKind::Alias(IsRigid::No, alias.clone()));
        let ct = f.fold_const(&tcx.tys, ct, 0);
        // eval_ctxt/mod.rs:1498-1513. generic_const_exprs is cut, but opaque
        // arguments still require evaluation in the caller's non-erased mode.
        if self.delegate.typing_mode.get().is_erased_not_coherence() && tcx.tys.has_opaque(tcx.tys.intern(TyKind::Const(ct))) {
            return Err(self.opaque_accesses.rerun_always());
        }
        if ct.has_non_region_infer(&tcx.tys) { return Ok(None); }
        let ConstKind::Alias(_, alias) = tcx.tys.const_kind(ct) else { unreachable!() };
        let typing_env = crate::tcx::TypingEnv { limit: self.delegate.recursion_limit, param_env: *env, mode: self.delegate.typing_mode.get() };
        match tcx.const_eval_resolve(self.solver.prog, typing_env, alias.kind.def_id(), &alias.args) {
            Ok(value) => Ok(Some(crate::consts::Const::new_value(&tcx.tys, value, self.const_type(&alias)?))),
            Err(crate::const_eval::ErrorHandled::TooGeneric) => Ok(None),
            Err(crate::const_eval::ErrorHandled::Reported(_)) => Ok(Some(tcx.tys.intern_const(ConstKind::Error))),
        }
    }
}
