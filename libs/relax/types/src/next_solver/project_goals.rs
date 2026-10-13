// ported from N:rustc_next_trait_solver/src/solve/project_goals/mod.rs @8d1a764
// ported from N:rustc_next_trait_solver/src/solve/project_goals/free_alias.rs @8d1a764
// ported from N:rustc_next_trait_solver/src/solve/project_goals/anon_const.rs @8d1a764
// ported from N:rustc_next_trait_solver/src/solve/normalizes_to.rs @8d1a764
use super::*;
use super::eval_ctxt::EvalCtxt;
use super::assembly::*;
use crate::types::{TyKind, AliasTyKind};
use crate::consts::{AliasConstKind, ConstKind, IsRigid};
impl EvalCtxt<'_, '_> {
    pub(super) fn compute_projection_goal(&mut self, env: ParamEnvId, p: ProjectionClause) -> QueryResultOrRerunNonErased {
        let associated = match &p.projection_term {
            AliasTerm::Ty(t) => match self.delegate.tcx.tys.kind(*t) {
                TyKind::Alias(_, alias) => match alias.kind {
                    AliasTyKind::Projection { .. } => true,
                    AliasTyKind::Opaque { .. } => return self.normalize_opaque_type(&env, &p),
                    AliasTyKind::Free { .. } => false,
                    AliasTyKind::Inherent { .. } => return self.normalize_inherent_associated_term(&env, &p),
                }, _ => unreachable!(),
            },
            AliasTerm::Const(a) => match a.kind {
                AliasConstKind::Projection { .. } => true,
                AliasConstKind::InherentSelf { .. } | AliasConstKind::InherentImpl { .. } => return self.normalize_inherent_associated_term(&env, &p),
                AliasConstKind::Free { .. } | AliasConstKind::Anon { .. } => false,
            }
        };
        if !associated {
            let (def, args) = self.alias_args(&p.projection_term);
            if !matches!(&p.projection_term, AliasTerm::Const(a) if matches!(a.kind, AliasConstKind::Anon { .. })) {
                self.add_item_clauses(&env, def, &args, GoalSource::Misc, false)?;
            }
            match &p.projection_term {
                AliasTerm::Ty(_) => { let ty = self.type_of(def, &args)?; let ty = self.normalize_ty(&env, ty)?; self.eq(&env, p.term, Term::Ty(ty))?; }
                AliasTerm::Const(a) => return self.evaluate_const_and_instantiate_projection_term(&env, p.term, a),
            }
            return self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes);
        }
        let term = self.next_term_infer_of_alias_kind(&p.projection_term);
        let goal = Goal { param_env: env.clone(), predicate: Predicate::dummy(PredicateKind::NormalizesTo(NormalizesTo { alias: p.projection_term, term })) };
        let (nested, evaluation) = self.evaluate_goal_raw(goal)?;
        self.eq(&env, p.term, term)?;
        for (source, goal) in nested { self.add_goal(source, goal)?; }
        self.evaluate_added_goals_and_make_canonical_response(evaluation.certainty)
    }
    pub(super) fn compute_normalizes_to_goal(&mut self, env: ParamEnvId, p: NormalizesTo) -> QueryResultOrRerunNonErased {
        let tr = self.alias_trait_ref(&p.alias);
        let (_, via) = self.probe().enter(|ecx| ecx.compute_trait_goal_with_source(&env, &TraitClause { trait_ref: tr.clone(), polarity: ClausePolarity::Positive }))?;
        let Some(via) = via else { return self.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS); };
        let from = if via == TraitGoalProvenVia::Misc { AssembleCandidatesFrom::All } else { AssembleCandidatesFrom::EnvAndBounds };
        let (mut candidates, _) = self.assemble_and_evaluate_candidates(&env, GoalKind::NormalizesTo(p.clone()), from)?;
        if !matches!(from, AssembleCandidatesFrom::All) {
            if candidates.is_empty() {
                self.eq(&env, p.term, self.rigid_alias_term(&p.alias))?;
                return self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes);
            }
            let (_, args) = self.alias_args(&p.alias);
            for &arg in &args[tr.args.len()..] {
                let term = match arg { GenericArg::Type(t) => Term::Ty(t), GenericArg::Const(c) => Term::Const(c), GenericArg::Lifetime(_) => continue };
                let term = self.structurally_normalize_term(&env, term)?;
                let infer = match term { Term::Ty(t) => matches!(self.delegate.tcx.tys.kind(t), TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_)), Term::Const(c) => matches!(self.delegate.tcx.tys.const_kind(c), ConstKind::Infer(_)) };
                if infer { return self.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS); }
            }
        }
        let mut has_param = false; for c in &candidates { has_param |= matches!(c.source, CandidateSource::ParamEnv(_)); }
        if has_param { candidates.retain(|c| matches!(c.source, CandidateSource::ParamEnv(_))); }
        match self.try_merge_candidates(&candidates) { Some((r,_)) => Ok(r), None => self.flounder(&candidates) }
    }
    pub(super) fn instantiate_normalizes_to_term(&mut self, env: &ParamEnvId, p: &NormalizesTo, value: Term) -> Result<(), NoSolutionOrRerunNonErased> {
        if let (AliasTerm::Const(alias), Term::Const(ct)) = (&p.alias, value) {
            self.add_goal(GoalSource::Misc, Goal { param_env: env.clone(), predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::ConstArgHasType(ct, self.const_type(alias)?))) })?;
        }
        let value = match value { Term::Ty(t) => Term::Ty(self.normalize_ty(env, t)?), value => value };
        self.eq(env, p.term, value)
    }
    pub(super) fn rigid_alias_term(&self, alias: &AliasTerm) -> Term {
        let tys = &self.delegate.tcx.tys;
        match alias {
            AliasTerm::Ty(t) => { let TyKind::Alias(_, a) = tys.kind(*t) else { unreachable!() }; Term::Ty(tys.intern(TyKind::Alias(IsRigid::Yes, a.clone()))) }
            AliasTerm::Const(a) => Term::Const(tys.intern_const(ConstKind::Alias(IsRigid::Yes, a.clone()))),
        }
    }
    pub(super) fn confirm_projection_impl(&mut self, env: &ParamEnvId, p: &NormalizesTo, imp: crate::program::DefId, impl_args: &[GenericArg]) -> Result<(), NoSolutionOrRerunNonErased> {
        self.try_evaluate_added_goals()?;
        let tcx = self.delegate.tcx;
        let (def, args) = self.alias_args(&p.alias); let trait_def = self.solver.prog.def(def).parent;
        self.add_item_clauses(env, def, &args, GoalSource::AliasWellFormed, true)?;
        let item = tcx.fetch_eligible_assoc_item(self.solver.prog, def, imp);
        if !tcx.associated_item_has_value(self.solver.prog, item) {
            if self.delegate.impl_self_is_guaranteed_unsized(self.solver.prog, imp) {
                if matches!(self.delegate.typing_mode, crate::tcx::TypingMode::Coherence) {
                    self.add_goal(GoalSource::Misc, Goal { param_env: env.clone(), predicate: Predicate::dummy(PredicateKind::Ambiguous) })?;
                    return Ok(());
                }
                return self.instantiate_normalizes_to_term(env, p, self.rigid_alias_term(&p.alias));
            }
            self.delegate.errors.delayed_bug(self.delegate.cause, "missing associated item definition".into());
            return self.instantiate_normalizes_to_error(env, p);
        }
        let target = if self.solver.prog.def(item).parent == trait_def { args.clone() } else {
            let parent = tcx.generic_indices_of(self.solver.prog, trait_def);
            let mut result = impl_args.to_vec(); result.extend_from_slice(&args[parent.lifetimes.len() + parent.types_consts.len()..]); result
        };
        let identity = tcx.generic_indices_of(self.solver.prog, item).identity_args(&tcx.tys);
        if target.len() != identity.len() || target.iter().zip(&identity).any(|(a, b)| std::mem::discriminant(a) != std::mem::discriminant(b)) {
            self.delegate.errors.delayed_bug(self.delegate.cause, "associated item has mismatched arguments".into());
            return self.instantiate_normalizes_to_error(env, p);
        }
        let value = match &p.alias {
            AliasTerm::Ty(_) => Term::Ty(self.type_of(item, &target)?),
            AliasTerm::Const(_) => {
                let alias = AliasConst { kind: AliasConstKind::Projection { def_id: item }, args: target.into() };
                if let Some(ct) = self.evaluate_const(env, &alias)? { Term::Const(ct) }
                else { self.add_goal(GoalSource::Misc, Goal { param_env: env.clone(), predicate: Predicate::dummy(PredicateKind::Ambiguous) })?; return Ok(()); }
            }
        };
        self.instantiate_normalizes_to_term(env, p, value)
    }
    fn instantiate_normalizes_to_error(&mut self, env: &ParamEnvId, p: &NormalizesTo) -> Result<(), NoSolutionOrRerunNonErased> {
        let value = match &p.alias {
            AliasTerm::Ty(_) => Term::Ty(self.delegate.tcx.tys.error),
            AliasTerm::Const(_) => Term::Const(self.delegate.tcx.tys.intern_const(ConstKind::Error)),
        };
        self.instantiate_normalizes_to_term(env, p, value)
    }
    pub(super) fn evaluate_const_and_instantiate_projection_term(&mut self, env: &ParamEnvId, expected: Term, alias: &AliasConst) -> QueryResultOrRerunNonErased {
        match self.evaluate_const(env, alias)? {
            Some(ct) => { self.eq(env, expected, Term::Const(ct))?; self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes) }
            None => self.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS),
        }
    }
    fn normalize_inherent_associated_term(&mut self, env: &ParamEnvId, p: &ProjectionClause) -> QueryResultOrRerunNonErased {
        let (def, mut args) = self.alias_args(&p.projection_term); let tcx = self.delegate.tcx;
        if !matches!(&p.projection_term, AliasTerm::Const(AliasConst { kind: AliasConstKind::InherentImpl { .. }, .. })) {
            let imp = self.solver.prog.def(def).parent;
            let mut impl_args = self.fresh_args_for_item(imp);
            let actual = tcx.instantiate_ty(self.solver.prog, imp, &impl_args, tcx.impl_self[&imp]);
            let GenericArg::Type(self_ty) = args[0] else { unreachable!() };
            self.eq(env, Term::Ty(self_ty), Term::Ty(actual))?;
            impl_args.extend_from_slice(&args[1..]); args = impl_args;
        }
        self.add_item_clauses(env, def, &args, GoalSource::Misc, false)?;
        match &p.projection_term {
            AliasTerm::Ty(_) => { let ty = self.type_of(def, &args)?; let ty = self.normalize_ty(env, ty)?; self.eq(env, p.term, Term::Ty(ty))?; }
            AliasTerm::Const(_) => return self.evaluate_const_and_instantiate_projection_term(env, p.term, &AliasConst { kind: AliasConstKind::InherentImpl { def_id: def }, args: args.into() }),
        }
        self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes)
    }
}

impl crate::infer::InferCtxt<'_> {
    /// rustc_ty_utils/src/ty.rs:348-400: only a normalized dyn/slice/str
    /// structural tail guarantees unsizedness; ambiguity is intentionally ignored.
    fn impl_self_is_guaranteed_unsized(&self, prog: &crate::program::Program, imp: DefId) -> bool {
        let infcx = crate::infer::InferCtxt::for_item(self.tcx, prog, crate::tcx::TypingMode::non_body_analysis(), imp);

        let env = self.tcx.param_env_id(prog, imp);
        let mut ocx = fulfill::ObligationCtxt::new(&infcx, prog);
        let mut ty = ocx.normalize_ty(&env, self.tcx.impl_self[&imp]);
        let mut depth = 0;
        loop {
            if depth >= infcx.recursion_limit(prog) {
                infcx.errors.report(infcx.cause, "overflow computing structural tail".into());
                return false;
            }
            depth += 1;
            let next = match self.tcx.tys.kind(ty) {
                TyKind::Adt(def, args, regions) if prog.def(*def).kind == crate::program::DefKind::Struct => {
                    let Some(field) = self.tcx.adts[def].variants[0].fields.last() else { break; };
                    self.tcx.instantiate_ty(prog, *def, &self.tcx.complete_args(prog, *def, self.tcx.tys.list(*args), *regions), field.ty)
                }
                TyKind::Tuple(types) if !types.is_empty() => *self.tcx.tys.list(*types).last().unwrap(),
                _ => break,
            };
            ty = ocx.normalize_ty(&env, next);
        }
        matches!(self.tcx.tys.kind(ty), TyKind::Dyn(..) | TyKind::Slice(_) | TyKind::Str)
    }
}
